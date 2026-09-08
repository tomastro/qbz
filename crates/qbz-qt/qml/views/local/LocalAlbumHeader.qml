// Local album header block (album/LocalAlbumView.slint:213-404): the 224px
// cover, the title / artist line with the "+N more artists" expander and its
// expanded list, the info line, the local action row (play / shuffle / edit
// tags / add to playlist / add to Mixtape) and the version picker.
//
// `artistsExpanded` is view-local and resets when another album opens — the
// Slint watches its own mirror property for exactly this (:156).

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../../controls"
import "../../theme"
import "../../kiosk"

Row {
    property bool kioskHost: false
    id: root

    property var album: null
    /// De-duplicated contributor list (>1 turns the expander on).
    property var allArtists: []
    property string infoLine: ""
    property string compactInfoLine: ""
    property bool compact: false
    property var versions: []
    property string coverSource: ""
    /// Per-item cover state, owned by the page (LocalAlbumView).
    /// `coverPending` means "this album EXPECTS a cover" — whether one has
    /// arrived is decided here, by the image's own readiness, never by the
    /// path being non-empty (see theme/RoundedImage.qml's contract).
    property bool coverPending: false
    /// Optional: true while the page's artwork pass is still in flight. It
    /// suspends the settle countdown so a slow cold decode cannot expire the
    /// placeholder a beat before the cover lands. Defaults to false, so the
    /// page need not supply it.
    property bool artPassActive: false
    property bool skelPhase: false
    property int artSettleMs: 0
    /// Header palette selected by LocalAlbumView from the same atmosphere /
    /// ambient rules as Qobuz AlbumView.
    property color strongColor: theme.textPrimary
    property color bodyColor: theme.textSecondary
    property bool overlay: false
    signal openArtist(string name)
    signal compactToggled()

    QbzTheme { id: theme }

    property bool artistsExpanded: false
    onAlbumChanged: artistsExpanded = false

    readonly property int coverPx: kioskHost ? 96 : (compact ? 112 : 224)
    spacing: kioskHost ? 16 : (compact ? 20 : 32)

    Rectangle {
        width: root.coverPx
        height: root.coverPx
        radius: 12
        color: theme.surfaceElevated
        clip: true
        RoundedImage {
            id: headerArt
            anchors.fill: parent
            source: root.kioskHost ? "" : root.coverSource
            radius: 12
        }
        Loader {
            anchors.fill: parent
            active: root.kioskHost
            sourceComponent: KioskArtwork { source: root.coverSource }
        }
        // Per-item: hands over on the paint, not the path; settles out when
        // the album has none (local artwork drops keys with no cover).
        QbzSkeleton {
            variant: "art"
            anchors.fill: parent
            blockRadius: 12
            pending: !root.kioskHost && root.coverPending
            coverReady: headerArt.ready
            phase: root.skelPhase
            settleMs: root.artSettleMs
            settleHold: root.artPassActive
        }
    }

    Column {
        width: parent.width - root.coverPx - root.spacing
        topPadding: 4
        spacing: 0

        Item {
            width: parent.width
            height: Math.max(localAlbumTitle.implicitHeight, 32)
            Text {
                id: localAlbumTitle
                visible: !root.compact
                anchors.left: parent.left
                anchors.right: localHeaderModeButton.left
                anchors.rightMargin: 12
                anchors.verticalCenter: parent.verticalCenter
                text: root.album ? (root.album.title || "") : ""
                color: root.strongColor
                font.pixelSize: root.kioskHost ? 22 : theme.fontSection
                font.weight: theme.weightBold
                elide: Text.ElideRight
            }
            Row {
                id: compactHeading
                visible: root.compact
                anchors.left: parent.left
                anchors.right: localHeaderModeButton.left
                anchors.rightMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 0
                Text {
                    id: compactAlbumTitle
                    width: !root.album || (root.album.artist || "") === ""
                        ? compactHeading.width
                        : Math.min(implicitWidth,
                                   Math.max(80, compactHeading.width * 0.62))
                    text: root.album ? (root.album.title || "") : ""
                    color: root.strongColor
                    font.pixelSize: root.kioskHost ? 22 : theme.fontSection
                    font.weight: theme.weightBold
                    elide: Text.ElideRight
                }
                Text {
                    id: compactTitleSeparator
                    visible: root.album && (root.album.artist || "") !== ""
                    text: "  •  "
                    color: root.bodyColor
                    font.pixelSize: root.kioskHost ? 22 : theme.fontSection
                    font.weight: theme.weightBold
                }
                Text {
                    width: Math.max(0, compactHeading.width
                        - compactAlbumTitle.width - compactTitleSeparator.width)
                    text: root.album ? (root.album.artist || "") : ""
                    color: compactArtistArea.containsMouse
                        ? root.strongColor : root.bodyColor
                    font.pixelSize: root.kioskHost ? 22 : theme.fontSection
                    font.weight: theme.weightBold
                    elide: Text.ElideRight
                    MouseArea {
                        id: compactArtistArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: if (root.album) root.openArtist(root.album.artist)
                    }
                }
            }
            QbzIconButton {
                id: localHeaderModeButton
                visible: !root.kioskHost
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.topMargin: -4
                btnSize: root.compact ? 22 : 26
                iconSize: root.compact ? 11 : 13
                name: root.compact ? "maximize-2" : "minimize-2"
                tintOverride: root.overlay ? "white" : ""
                onClicked: root.compactToggled()
                HoverHandler { id: localHeaderModeHover }
                ToolTip.visible: localHeaderModeHover.hovered
                ToolTip.text: root.compact
                    ? QbzSession.tr("Show full album header", QbzSession.trRev)
                    : QbzSession.tr("Show compact album header", QbzSession.trRev)
                ToolTip.delay: 350
            }
        }
        Item { width: 1; height: root.compact ? 2 : 4 }
        Row {
            visible: !root.compact
            spacing: 8
            Text {
                text: root.album ? (root.album.artist || "") : ""
                color: artistArea.containsMouse ? root.strongColor : root.bodyColor
                font.pixelSize: theme.fontHeading
                font.weight: theme.weightBold
                MouseArea {
                    id: artistArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.openArtist(root.album.artist)
                }
            }
            // Multi-artist album (compilations): "+N more artists" expands
            // the full track-artist list below.
            Text {
                visible: !root.compact && root.allArtists.length > 1
                anchors.verticalCenter: parent.verticalCenter
                text: root.artistsExpanded
                    ? QbzSession.tr("Show less", QbzSession.trRev)
                    : "+" + (root.allArtists.length - 1) + " "
                      + QbzSession.tr("more artists", QbzSession.trRev)
                color: moreArea.containsMouse ? root.strongColor : root.bodyColor
                font.pixelSize: theme.fontBody
                MouseArea {
                    id: moreArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: root.artistsExpanded = !root.artistsExpanded
                }
            }
        }
        QbzSelect {
            visible: root.kioskHost && root.allArtists.length > 1
            kioskHost: true
            width: parent.width
            options: root.allArtists
            searchable: options.length > 8
            onSelected: function(i) { root.openArtist(root.allArtists[i]) }
        }
        // Expanded track-artist list — each entry routes to the Local
        // Library Artists tab (name-based).
        Column {
            visible: !root.kioskHost && !root.compact && root.artistsExpanded
                     && root.allArtists.length > 1
            width: parent.width
            topPadding: 6
            spacing: 0
            Repeater {
                model: root.kioskHost ? [] : root.allArtists
                delegate: Text {
                    id: nameRow
                    required property string modelData
                    height: 24
                    text: modelData
                    color: nameArea.containsMouse ? theme.accent : root.bodyColor
                    font.pixelSize: theme.fontBody
                    verticalAlignment: Text.AlignVCenter
                    MouseArea {
                        id: nameArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.openArtist(nameRow.modelData)
                    }
                }
            }
        }
        Item { width: 1; height: root.compact ? 2 : 10 }
        Text {
            width: parent.width
            text: root.compact ? root.compactInfoLine : root.infoLine
            color: root.bodyColor
            font.pixelSize: theme.fontBody
            elide: Text.ElideRight
        }
        // Kiosk-only album quality badge (owner feedback 2026-09-07: on the
        // panel the quality was unknown until a track played). The album row
        // carries the same tier/detail the cards badge; Rust's "max"/"lossy"
        // spellings fold into the four names the badge draws, as
        // controls/QualityBadge.qml does. Desktop keeps its layout untouched.
        Item {
            visible: root.kioskHost && kioskQuality.tier !== ""
            width: parent.width
            height: visible ? kioskQuality.implicitHeight + 8 : 0
            QualityBadgeFull {
                id: kioskQuality
                readonly property string rawTier: root.album ? (root.album.qualityTier || "") : ""
                tier: rawTier === "max" ? "hires" : rawTier === "lossy" ? "mp3" : rawTier
                detail: root.album ? (root.album.qualityDetail || "") : ""
                showIcon: true
                scaleFactor: 1.15
                anchors.verticalCenter: parent.verticalCenter
            }
        }
        Item { width: 1; height: root.compact ? 10 : 20 }

        // ---- Local action row ----
        Row {
            spacing: 12
            QbzCircleAction {
                name: "play-fill"
                primary: true
                compactPrimary: root.compact
                diameterOverride: root.kioskHost ? 44 : (root.compact ? 28 : 0)
                overlay: root.overlay
                onClicked: QbzLocal.albumSelectedAction("play", "")
            }
            QbzCircleAction {
                name: "shuffle"
                diameterOverride: root.kioskHost ? 44 : (root.compact ? 28 : 0)
                overlay: root.overlay
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzLocal.albumSelectedAction("shuffle", "")
            }
            // ASSET GAP: Slint uses `pencil`; the Qt set ships pen-line.
            QbzCircleAction {
                name: "pen-line"
                diameterOverride: root.kioskHost ? 44 : (root.compact ? 28 : 0)
                overlay: root.overlay
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzLocal.albumEditTags(root.album.id)
            }
            QbzCircleAction {
                name: "list-plus"
                diameterOverride: root.kioskHost ? 44 : (root.compact ? 28 : 0)
                overlay: root.overlay
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzLocal.albumAddToPlaylist(root.album.id)
            }
            QbzCircleAction {
                name: "cassette-tape"
                diameterOverride: root.kioskHost ? 44 : (root.compact ? 28 : 0)
                overlay: root.overlay
                anchors.verticalCenter: parent.verticalCenter
                onClicked: QbzLocal.albumAddToMixtape(root.album.id)
            }
        }

        // ---- Version picker (multiple physical copies only) ----
        Item {
            visible: !root.compact && root.versions.length > 1
            width: 1
            height: visible ? 16 : 0
        }
        Row {
            visible: !root.compact && root.versions.length > 1
            spacing: 8
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.versions.length + " "
                    + QbzSession.tr("versions", QbzSession.trRev) + ":"
                color: root.bodyColor
                font.pixelSize: theme.fontLegal
            }
            VersionPicker { kioskHost: root.kioskHost;
                anchors.verticalCenter: parent.verticalCenter
                versions: root.versions
                current: root.album ? (root.album.versionIndex || 0) : 0
                overlay: root.overlay
                onPicked: function (i) { QbzLocal.albumSelectVersion(i) }
            }
        }
    }
}
