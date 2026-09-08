// PmGridCard — the GRID-mode playlist cell (PlaylistManagerView.slint:470-618).
// 160 wide; 220 tall, or 250 when the reorder header shows. r8, `surface-card`
// → `surface-hover` on BODY hover (the footer buttons are outside the body
// area, exactly like the reference's `ta`), 0.6 opacity while hidden.
//
// Affordances (contract D27): open (body), favourite, visibility,
// edit-playlist, add-to-mixtape, move-up, move-down. NO move-to-folder — that
// one exists only on the list row. The four footer buttons are ALWAYS visible;
// there is no hover reveal, no play affordance, no pin badge and no overlay row
// on this card (that is cards/PlaylistCard.qml, a different component).
//
// TWO local-row suppressions, both contract decisions:
//   * D28 — the cassette is NOT RENDERED on a `local:` row. `accepts_item`
//     answers `Accepts::NONE` for (Playlist, Local) and `qbz_mixtape::enqueue`
//     hard-errors, so the row can never resolve; a dimmed control would promise
//     "temporarily unavailable". `Row` skips invisible children, so the footer
//     simply narrows.
//   * D29 — the reorder chevrons are NOT RENDERED on a `local:` row (custom
//     positions are u64-keyed and `reorder_step` drops locals), but the 22 px
//     header IS still reserved: `canReorder` switches EVERY card 220 → 250
//     globally, so hiding the header would leave a 30 px hole in this one card.
//
// The grip glyph between the chevrons is DECORATIVE — no MouseArea. The
// reference has no drag and drop anywhere in this view (contract D30) and the
// counter row says so in words ("Use the arrows to reorder playlists").

import QtQuick
import "../../kiosk"
import com.blitzfc.qbz
import "../../cards"
import "../../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    /// One `PlaylistRow` (contract §4.3).
    property var item: ({})
    /// ABSOLUTE index in the visible order (the grid delegate is a ROW of
    /// cards, so the Repeater's own index is the column — D1).
    property int itemIndex: 0
    /// Drives BOTH the 220 → 250 height and the reserved header (D29).
    property bool canReorder: false

    QbzTheme { id: theme }

    readonly property string itemId: String(root.item.id || "")
    readonly property bool isLocal: root.item.isLocal === true

    width: root.kioskHost ? 200 : 160
    height: root.kioskHost ? (root.canReorder ? 320 : 270) : (root.canReorder ? 250 : 220)
    radius: 8
    color: bodyArea.containsMouse ? theme.surfaceHover : theme.surfaceCard
    opacity: (root.item.isHidden === true) ? 0.6 : 1.0

    Column {
        x: 10
        y: 10
        width: root.kioskHost ? 180 : 140
        spacing: 8

        // Reorder header — the SLOT is reserved whenever `canReorder`, the
        // controls only render on a Qobuz row (D29).
        Item {
            visible: root.canReorder
            width: root.kioskHost ? 180 : 140
            height: root.kioskHost ? 44 : 22

            PmActionButton {
                visible: !root.isLocal
                anchors.left: parent.left
                name: "chevron-up"
                btnSize: root.kioskHost ? 44 : 22
                glyph: 14
                onClicked: QbzPlaylistManager.moveUp(root.itemId)
            }
            QbzIcon {
                visible: !root.isLocal
                anchors.centerIn: parent
                name: "grip-vertical"
                width: 13
                height: 13
                tintName: "muted"
            }
            PmActionButton {
                visible: !root.isLocal
                anchors.right: parent.right
                name: "chevron-down"
                btnSize: root.kioskHost ? 44 : 22
                glyph: 14
                onClicked: QbzPlaylistManager.moveDown(root.itemId)
            }
        }

        // Artwork + availability badge.
        Item {
            width: root.kioskHost ? 180 : 140
            height: root.kioskHost ? 180 : 140

            Loader {
                anchors.fill: parent
                sourceComponent: root.kioskHost ? kioskCover : desktopCover
                Component {
                    id: kioskCover
                    KioskArtwork {
                        anchors.fill: parent
                        source: (root.item.covers || [])[0] || ""
                        radius: 8
                    }
                }
                Component {
                    id: desktopCover
                    PlaylistCollage {
                        anchors.fill: parent
                        layout: "pm"
                        urls: root.item.covers || []
                        radius: 8
                    }
                }
            }
            PmLocalBadge {
                x: parent.width - width - 6
                y: 6
                status: root.item.localStatus || ""
                isLocal: root.isLocal
                offlineOnly: root.item.offlineOnly === true
            }
        }

        Text {
            width: root.kioskHost ? 180 : 140
            height: 18
            text: root.item.name || ""
            color: theme.textPrimary
            font.pixelSize: 13
            font.weight: theme.weightMedium
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }

        // Footer: meta on the left, the action strip on the right.
        Item {
            width: root.kioskHost ? 180 : 140
            height: root.kioskHost ? 44 : 22

            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                width: Math.max(0, parent.width - actions.width)
                // Pluralised in RUST (`qbz_i18n::tf`) — QML never pluralises.
                text: root.item.tracksLine || ""
                color: theme.textMuted
                font.pixelSize: 11
                elide: Text.ElideRight
            }
            Row {
                id: actions
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: 0

                PmActionButton {
                    name: root.item.isFavorite === true ? "heart-filled" : "heart"
                    filled: root.item.isFavorite === true
                    btnSize: root.kioskHost ? 44 : 24
                    glyph: 14
                    onClicked: QbzPlaylistManager.toggleFavorite(root.itemId)
                }
                PmActionButton {
                    name: root.item.isHidden === true ? "eye-off" : "eye"
                    btnSize: root.kioskHost ? 44 : 24
                    glyph: 14
                    onClicked: QbzPlaylistManager.toggleHidden(root.itemId)
                }
                PmActionButton {
                    name: "pen-line"
                    btnSize: root.kioskHost ? 44 : 24
                    glyph: 14
                    // The SHARED editor singleton (contract D2/D20) — the id
                    // lives in Rust from here on, so a republish can never make
                    // the modal save under a different playlist.
                    onClicked: QbzPlaylistEdit.open(root.itemId)
                }
                PmActionButton {
                    visible: !root.isLocal
                    name: "cassette-tape"
                    btnSize: root.kioskHost ? 44 : 24
                    glyph: 14
                    // The payload is assembled AT THE CALL SITE (D3) — the same
                    // seam every other surface uses. `source` is always "qobuz"
                    // because this button is absent on a local row (D28).
                    onClicked: QbzMyQbzAdd.open(JSON.stringify([{
                        "itemType": "playlist", "source": "qobuz",
                        "sourceItemId": root.itemId,
                        "title": root.item.name || "",
                        "subtitle": "", "artworkUrl": (root.item.covers || [])[0] || "",
                        "year": null, "trackCount": root.item.totalCount || 0
                    }]))
                }
            }
        }
    }

    // Body hit area: artwork + name only (pad 10 + collage 140 + gap 8 + name
    // 18 = 176), below the reorder header and above the footer, so both keep
    // their own clicks (`:605-617`).
    MouseArea {
        id: bodyArea
        y: root.canReorder ? (root.kioskHost ? 52 : 30) : 0
        width: parent.width
        height: root.kioskHost ? 216 : 176
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        // Routes `local:` ids to the local loader and works OFFLINE
        // (main.rs:926-955). There is no manager-local twin (D3).
        onClicked: QbzBridge.openPlaylist(root.itemId)
    }
}
