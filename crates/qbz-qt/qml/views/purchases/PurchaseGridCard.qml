// PurchaseGridCard — one album cell of the Purchases grid. 1:1 port of
// purchases/PurchasesView.slint's `PurchaseGridCard` (:230-300).
//
// SELF-CONTAINED ON PURPOSE, and the .slint records the same call in its own
// header: the Purchases card is "card-lite" — cover / title / artist / quality,
// NO play overlay, no heart, no pin badge, no ⋯ menu — and it carries an
// UNAVAILABLE state that `cards/AlbumCard.qml` has no arm for at all.
// Threading two Purchases-only flags through the shared card would also have to
// go through `views/AlbumCollection.qml`, which mounts AlbumCard with ~15
// hardcoded properties, exposes no signals, and routes every click to
// `QbzAlbum.openAlbum` — i.e. straight to the CATALOG album page, which would
// leave the purchase-album route (§G.1: `QbzPurchases.openAlbum` then
// `QbzShell.navigateTo("purchase-album")`) unreachable. So the cell is its own
// file, exactly as the .slint decided for exactly these reasons.
//
// 162x232: a 1:1 cover (Radius.sm well, surface-elevated, disc-3 placeholder)
// over the title / artist / quality lines.
//
// UNAVAILABLE (`downloadable == false`): the cell dims to .45 (.55 on hover)
// and an alpha-60 scrim with the "Unavailable" label covers the artwork. The
// click is gated on it too — an unavailable purchase must not open a detail
// page that has nothing to offer. No `Behavior` on the opacity: it is fed by
// data and by hover, which is precisely the shape the repaint-pulse rule
// forbids animating (the .slint animates neither).
//
// There is deliberately NO "downloaded" mark here. Neither Tauri nor the
// .slint draws one on an album cell; `downloaded` feeds the "Hide downloaded"
// filter and the detail screen, and inventing a badge would be a behaviour
// this screen never had.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Item {
    id: root

    /// One row of `list_json.albums` (§G.2).
    property var album: ({})
    /// Viewport artwork resolver override. Empty falls back to the path that
    /// was already cached when Rust built the document.
    property string artSource: ""
    signal clicked()

    QbzTheme { id: theme }

    readonly property bool avail: root.album.downloadable !== false
    /// `artPath`, NEVER `artworkUrl`. The url is the REMOTE cover and this port
    /// does not fetch covers from a delegate — Rust resolves them onto disk and
    /// publishes the `file://` path, `""` until it lands. A cell handed only the
    /// url draws nothing at all (purchases_qt.rs::AlbumRow::art_path, and the
    /// same trap documented in artist_releases_qt.rs and musician_qt.rs).
    readonly property string artUrl: root.artSource !== ""
        ? root.artSource : (root.album.artPath || "")

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // .slint:238 — .45 / .55 on hover / 1.0.
    opacity: root.avail ? 1.0 : (cardArea.containsMouse ? 0.55 : 0.45)

    Column {
        width: parent.width
        spacing: 8

        // --- Cover, 1:1 ---------------------------------------------------
        Rectangle {
            id: cover
            width: parent.width
            height: parent.width
            radius: theme.radiusSm
            color: theme.surfaceElevated
            // No clip: RoundedImage confines its own crop and the scrim below
            // is geometrically contained (the AlbumCard rule — one batch root
            // per cell, no scissor that rounds nothing).

            RoundedImage {
                id: art
                anchors.fill: parent
                source: root.artUrl
                radius: theme.radiusSm
            }
            // .slint:252 — the empty-well glyph. Gated on `ready`, not on "the
            // path is non-empty": a url that is still decoding (or that failed)
            // must not leave a bare square (RoundedImage's readiness contract).
            QbzIcon {
                visible: !art.ready
                name: "disc-3"
                width: 36
                height: 36
                anchors.centerIn: parent
                tintName: "muted"
            }

            // .slint:263-278 — the unavailable scrim over the artwork.
            Rectangle {
                visible: !root.avail
                anchors.fill: parent
                radius: theme.radiusSm
                color: theme.alphaTier(60)
                Text {
                    anchors.centerIn: parent
                    width: parent.width - 12
                    text: root.t("Unavailable")
                    color: theme.textPrimary
                    font.pixelSize: theme.fontLegal
                    font.weight: theme.weightSemibold
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                }
            }
        }

        // --- Title / artist + quality badge ------------------------------
        // The SAME icon-only badge every other album card draws
        // (cards/AlbumCard.qml): tier glyph, exact quality in its tooltip.
        // The .slint printed the raw tier word ("hires") as a third text
        // line, which is the one thing on this page no other surface does.
        Row {
            width: parent.width
            height: 40
            spacing: theme.spacingSm
            Column {
                width: parent.width - (qBadge.visible ? qBadge.width + theme.spacingSm : 0)
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2
                Text {
                    width: parent.width
                    height: 20
                    text: root.album.title || ""
                    color: theme.textPrimary
                    font.pixelSize: theme.fontBody
                    font.weight: theme.weightMedium
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
                Text {
                    width: parent.width
                    height: 17
                    text: root.album.artist || ""
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                    verticalAlignment: Text.AlignVCenter
                    elide: Text.ElideRight
                }
            }
            QualityMini {
                id: qBadge
                tier: root.album.qualityTier || ""
                label: root.album.qualityDetail || ""
                anchors.verticalCenter: parent.verticalCenter
            }
        }
    }

    MouseArea {
        id: cardArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: root.avail ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: if (root.avail) root.clicked()
    }
}
