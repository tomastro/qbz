// ReactiveSplitPanel — the two cinematic SPLIT variants requested from the
// 2026-08-27 reference frame + annotated owner revision. Both share the live
// Immersive ambient stage, one large album-blurred card and an upward WaveBed.
// The standard variant keeps title, artist and an always-present SECOND seek
// bar inside the card; lyricsMode
// replaces that entire metadata/seek block with the current lyric, one line at
// a time. The ordinary ImmersivePlayerBar remains an independent layer.

import QtQuick
import QtQuick.Effects
import QtQuick.Window
import com.blitzfc.qbz
import "../controls"
import "../shell"
import "../theme"

Item {
    id: root

    property bool lyricsMode: false
    property var lines: []
    property bool synced: false
    property int status: 0
    property var sync: null

    readonly property int activeIndex: root.sync ? root.sync.activeIndex : -1
    readonly property string currentLine:
        (root.activeIndex >= 0 && root.activeIndex < root.lines.length
         && root.lines[root.activeIndex].text !== undefined)
            ? root.lines[root.activeIndex].text : ""
    readonly property bool waiting: root.synced && root.status === 2
        && root.lines.length > 0 && root.currentLine === ""
    readonly property bool noLyrics: root.status === 3
        || (root.status === 2 && root.lines.length === 0)

    readonly property string artSource: QbzPlayer.npArtworkPathLarge !== ""
        ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    // Geometry from the owner's annotated 1280x720 composition. `stageTop` is
    // below the header; `playerTop` is the fixed y of ImmersivePlayerBar. The
    // card consumes 70% of that useful height and 85% of the viewport width.
    // Portrait/narrow windows cap height against width so the text column can
    // never be eaten by the square artwork.
    readonly property real stageTop: Math.min(72,
        Math.max(56, root.height * 0.09))
    readonly property real playerTop: Math.max(root.stageTop + 160,
        root.height - 114)
    readonly property real stageHeight: Math.max(160,
        root.playerTop - root.stageTop)
    readonly property real cardWidth: Math.max(1,
        Math.min(root.width - 32, root.width * 0.85))
    readonly property real cardHeight: Math.max(120,
        Math.min(root.stageHeight * 0.70, root.cardWidth * 0.56))
    readonly property real cardTop: root.stageTop
        + Math.max(0, root.stageHeight - root.cardHeight) * 0.55
    // Same 16px curve as the NPB, shared by the outer card and artwork.
    readonly property real cardRadius: 16
    readonly property real cardArtSize: Math.min(root.cardHeight * 0.94,
        root.cardWidth * 0.42)

    readonly property real waveBottom: root.playerTop - 8
    // Consume 80% of the REAL card-to-NPB slot. The remaining 20% is the air
    // above the WaveBed; the 8px below is its clearance from the player bar.
    readonly property real waveRoom: Math.max(2, root.waveBottom
        - (root.cardTop + root.cardHeight))
    readonly property real waveHeight: Math.max(2,
        root.waveRoom * 0.80)
    readonly property int waveBars: 48

    // The card's blurred cover is deliberately oversized before it is
    // sampled. No cover edge therefore coincides with the glass-card edge.
    readonly property real cardBackdropOverscan: Math.max(36,
        Math.min(root.cardWidth, root.cardHeight) * 0.12)

    // The album's OWN dominant hue, lifted for legibility (AmbientAccent).
    // This used to be the exact COMPLEMENT of the ambient primary, which is
    // how a deep-blue cover got an orange waveform/seek — the accent now
    // stays in the album palette (2026-08-31 visual cleanup). Unlike FOCUS
    // Wave Bed this is not mirrored: every column grows upward from one
    // baseline.
    AmbientAccent { id: ambientAccent }
    readonly property color waveColor: ambientAccent.value

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
    readonly property string lyricFontFamily:
        selectedFontLoader.item
        && selectedFontLoader.item.status === FontLoader.Ready
            ? selectedFontLoader.item.name : ""

    function fmt(sec) {
        sec = Math.max(0, Math.floor(sec))
        var m = Math.floor(sec / 60)
        var s = sec % 60
        return m + ":" + (s < 10 ? "0" : "") + s
    }

    function requestArtSize() {
        if (root.visible)
            QbzPlayer.requestNpArtworkSize(Math.round(Math.min(960,
                Math.max(160, root.cardArtSize))))
    }
    onVisibleChanged: requestArtSize()
    onWidthChanged: requestArtSize()
    onHeightChanged: requestArtSize()
    Component.onCompleted: requestArtSize()

    // The lower complementary WaveBed is a visualizer, not the seek bar. Its
    // 48 wider delegates are permanent; only their height bindings move.
    VizSettle {
        id: waveformSettle
        live: root.visible && QbzImmersive.open
        easeK: 0.62
        target: {
            var samples = QbzViz.waveform
            var out = new Array(root.waveBars)
            for (var i = 0; i < root.waveBars; ++i) {
                var at = Math.floor(i * 256 / root.waveBars)
                var value = samples[at] || 0
                if (value < 0)
                    value = -value
                out[i] = Math.min(1, value)
            }
            return out
        }
    }

    Item {
        id: waveform
        anchors.horizontalCenter: parent.horizontalCenter
        y: root.waveBottom - height
        width: root.cardWidth * 0.98
        height: root.waveHeight

        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 1
            color: Qt.rgba(root.waveColor.r, root.waveColor.g,
                root.waveColor.b, 0.50)
        }
        Repeater {
            model: root.waveBars
            delegate: Rectangle {
                required property int index
                readonly property real slot: waveform.width / root.waveBars
                x: index * slot + (slot - width) / 2
                anchors.bottom: parent.bottom
                width: Math.max(3, slot * 0.76)
                height: Math.max(2, waveformSettle.at(index)
                    * (waveform.height - 1))
                radius: Math.min(3, width / 2)
                color: Qt.rgba(root.waveColor.r, root.waveColor.g,
                    root.waveColor.b, index % 5 === 0 ? 1.0 : 0.82)
            }
        }
    }

    // The ambient field remains visible OUTSIDE this card. Inside, an opaque
    // crop of the actual cover is blurred and then darkened into glass. The
    // cover therefore shows through the glass, but the animated ambient layer
    // behind the card never does.
    Item {
        id: card
        x: (root.width - width) / 2
        y: root.cardTop
        width: root.cardWidth
        height: root.cardHeight
        // A post-effect rectangular clip is intentional here: the background
        // already has a rounded mask, while this hard boundary guarantees that
        // neither its blur nor the artwork shadow can leak below the card.
        clip: true

        Rectangle {
            anchors.fill: parent
            radius: root.cardRadius
            color: "#ee10171f"
        }

        // Rounded mask for the GPU blur arm. `radius + clip` does not clip QML
        // children to a curve, so use the same measured MultiEffect mask idiom
        // as RoundedImage and MiniShell.
        Item {
            id: cardBackdropMask
            anchors.fill: parent
            visible: false
            layer.enabled: !root._noShaders && root.artSource !== ""
            layer.smooth: true
            Rectangle {
                anchors.fill: parent
                radius: root.cardRadius
                color: "#ffffff"
            }
        }
        Item {
            id: cardBackdropClip
            anchors.fill: parent
            visible: !root._noShaders && root.artSource !== ""
            clip: true
            layer.enabled: visible
            layer.smooth: true
            layer.effect: MultiEffect {
                // Mask in a SECOND, outer pass. MultiEffect applies its blur
                // after sampling the source, so a combined blur+mask can bleed
                // a few pixels back into the rounded corners. This pass is the
                // final compositor and therefore contains the blur completely.
                maskEnabled: true
                maskSource: cardBackdropMask
                maskThresholdMin: 0.5
                maskSpreadAtMin: 1.0
            }
            Item {
                anchors.fill: parent
                clip: true
                layer.enabled: cardBackdropClip.visible
                layer.smooth: true
                layer.effect: MultiEffect {
                    blurEnabled: true
                    blurMax: 64
                    blur: 0.58
                }
                Image {
                    anchors.fill: parent
                    anchors.margins: -root.cardBackdropOverscan
                    source: root.artSource
                    asynchronous: true
                    cache: true
                    smooth: true
                    mipmap: true
                    fillMode: Image.PreserveAspectCrop
                }
            }
        }
        // Software/Null renderers cannot execute MultiEffect. Keep the source
        // contract (the cover, never Ambient) and retain rounded corners; only
        // the blur itself degrades.
        RoundedImage {
            anchors.fill: parent
            radius: root.cardRadius
            source: root.artSource
            visible: root._noShaders && root.artSource !== ""
        }
        Rectangle {
            anchors.fill: parent
            radius: root.cardRadius
            color: "#b80a1017"
        }
        Rectangle {
            anchors.fill: parent
            radius: root.cardRadius
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#22000000" }
                GradientStop { position: 0.55; color: "#7a090e14" }
                GradientStop { position: 1.0; color: "#bd070b10" }
            }
        }

        readonly property real artInset: (height - root.cardArtSize) / 2
        readonly property real contentGap: Math.max(20, width * 0.032)
        readonly property real contentRight: Math.max(20, width * 0.032)

        // Static artwork shadow, then the responsive 94%-height square.
        Rectangle {
            x: card.artInset
            y: card.artInset + 8
            width: root.cardArtSize
            height: root.cardArtSize
            radius: root.cardRadius
            color: "#99000000"
            layer.enabled: !root._noShaders
            layer.effect: MultiEffect {
                blurEnabled: true
                blurMax: 32
                blur: 0.55
            }
        }
        Rectangle {
            x: card.artInset
            y: card.artInset
            width: root.cardArtSize
            height: root.cardArtSize
            radius: root.cardRadius
            color: "#1cffffff"
            visible: root.artSource === ""
        }
        RoundedImage {
            x: card.artInset
            y: card.artInset
            width: root.cardArtSize
            height: root.cardArtSize
            radius: root.cardRadius
            source: root.artSource
            visible: root.artSource !== ""
        }

        Item {
            id: cardContent
            anchors.left: parent.left
            anchors.leftMargin: card.artInset + root.cardArtSize
                + card.contentGap
            anchors.right: parent.right
            anchors.rightMargin: card.contentRight
            anchors.top: parent.top
            anchors.topMargin: card.artInset
            anchors.bottom: parent.bottom
            anchors.bottomMargin: card.artInset

            // Standard now-playing card. Its metadata is one left-aligned
            // typographic unit whose centre lands exactly on the artwork's
            // vertical centre. The independent seek remains at the bottom.
            Item {
                id: nowPlayingPane
                anchors.fill: parent
                visible: !root.lyricsMode

                // Fixed nominal faces, clamped by the CARD's height only. The
                // shared shrink-to-fit typeScale is retired (owner, 2026-08-31
                // round 3): a long title now WRAPS at full size instead of
                // scaling the whole block down, and the subordinate lines
                // elide rather than dictate anyone's size.
                readonly property real nominalTitleSize: Math.max(24,
                    Math.min(48, card.height * 0.105))
                readonly property real nominalMetaSize: Math.max(13,
                    Math.min(21, card.height * 0.052))

                Column {
                    id: metadataStack
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 7

                    // "Now Playing" indicator at 1.5x, tinted with the
                    // album-palette accent instead of the theme accent so it
                    // matches the waveform/seek (owner request 2026-08-31).
                    Item {
                        visible: QbzPlayer.npPlaying
                        width: parent.width
                        height: visible ? 21 : 0
                        Row {
                            anchors.left: parent.left
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 12
                            EqualizerBars {
                                tint: root.waveColor
                                barScale: 1.5
                                active: root.visible && QbzImmersive.open
                                anchors.verticalCenter: parent.verticalCenter
                            }
                            Text {
                                text: QbzSession.tr("Now Playing",
                                    QbzSession.trRev)
                                color: root.waveColor
                                font.pixelSize: 18
                                font.weight: Font.DemiBold
                                font.letterSpacing: 0.5
                                anchors.verticalCenter: parent.verticalCenter
                            }
                        }
                    }

                    // Three metadata lines: title / album / artist. The title
                    // wraps at its full size instead of shrinking to fit.
                    Text {
                        width: parent.width
                        text: QbzPlayer.npTitle
                        color: "#f5ffffff"
                        font.pixelSize: nowPlayingPane.nominalTitleSize
                        font.weight: Font.Bold
                        wrapMode: Text.WordWrap
                    }
                    Text {
                        visible: QbzPlayer.npAlbum !== ""
                        width: parent.width
                        text: QbzPlayer.npAlbum
                        color: "#8cffffff"
                        font.italic: true
                        font.pixelSize: nowPlayingPane.nominalMetaSize
                        wrapMode: Text.NoWrap
                        elide: Text.ElideRight
                    }
                    Text {
                        visible: QbzPlayer.npArtist !== ""
                        width: parent.width
                        text: QbzPlayer.npArtist
                        color: "#b3ffffff"
                        font.pixelSize: nowPlayingPane.nominalMetaSize
                        wrapMode: Text.NoWrap
                        elide: Text.ElideRight
                    }

                    Item {
                        visible: QbzPlayer.npQualityTier !== ""
                        width: parent.width
                        height: visible ? qualityBadge.implicitHeight : 0
                        QualityBadgeFull {
                            id: qualityBadge
                            anchors.left: parent.left
                            compact: true
                            overAmbient: true
                            scaleFactor: 1.5
                            tier: QbzPlayer.npQualityTier
                            detail: QbzPlayer.npQualityDetail
                        }
                    }
                }

                // Always mounted and never tied to the NPB/chrome visibility.
                TinyBar {
                    id: secondSeek
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: seekTimes.top
                    anchors.bottomMargin: 2
                    value: QbzPlayer.npProgress
                    fill: root.waveColor
                    enabled: QbzPlayer.npDurationSecs > 0
                    onChanged: function (v) {
                        QbzPlayer.seek(Math.min(v, QbzPlayer.npSeekableMax))
                    }
                }
                Item {
                    id: seekTimes
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.bottom: parent.bottom
                    height: Math.max(10, card.height * 0.07)
                    Text {
                        anchors.left: parent.left
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.fmt(QbzPlayer.npElapsedSecs)
                        color: Qt.rgba(root.waveColor.r, root.waveColor.g,
                            root.waveColor.b, 0.82)
                        font.pixelSize: Math.max(10,
                            Math.min(13, card.height * 0.035))
                    }
                    Text {
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.fmt(QbzPlayer.npDurationSecs)
                        color: "#70ffffff"
                        font.pixelSize: Math.max(10,
                            Math.min(13, card.height * 0.035))
                    }
                }
            }

            // Lyric-card variant: the title, artist and second seek block are
            // replaced as one unit by exactly one synced lyric entry.
            Item {
                anchors.fill: parent
                visible: root.lyricsMode

                LyricsLineRow {
                    visible: root.currentLine !== ""
                    anchors.verticalCenter: parent.verticalCenter
                    contentWidth: parent.width
                    lineText: root.currentLine
                    lineIndex: root.activeIndex
                    sync: root.sync
                    synced: root.synced
                    activeIndex: root.activeIndex
                    sizeInactive: Math.max(22,
                        Math.min(44, card.height * 0.105))
                    sizeActive: sizeInactive
                    uppercase: QbzLyrics.uppercase
                    fontFamily: root.lyricFontFamily
                    activeColor: "#ffffff"
                    // 35% white so the karaoke fill reads (see
                    // LyricsFocusPanel.qml).
                    inactiveColor: "#59ffffff"
                    liteFill: false
                    centered: false
                    textShadow: true
                }
                Text {
                    visible: root.currentLine === "" && root.waiting
                    anchors.centerIn: parent
                    text: "♪"
                    color: "#70ffffff"
                    font.pixelSize: Math.max(32, card.height * 0.13)
                }
                Text {
                    visible: root.currentLine === "" && !root.waiting && root.noLyrics
                    anchors.fill: parent
                    text: QbzSession.tr("No lyrics", QbzSession.trRev)
                    color: "#80ffffff"
                    font.pixelSize: Math.max(17,
                        Math.min(26, card.height * 0.07))
                    font.italic: true
                    wrapMode: Text.WordWrap
                    verticalAlignment: Text.AlignVCenter
                }
            }
        }

        Rectangle {
            anchors.fill: parent
            radius: root.cardRadius
            color: "transparent"
            border.width: 1
            border.color: "#32ffffff"
        }
    }
}
