// ArtistCard — THE artist card (discover/ArtistGridCard.slint), shared by
// every surface since phase 21: Home/Editor/ForYou artist rails, the
// Pinned rail, the Library artist grids + All feed, the Search all-tab
// carousels + artists grid + most-popular artist hero. (The Slint mounts
// ArtistGridCard at 200x246 in ALL of these — the POC's old 160x220
// Home/Search cards replicated the legacy ArtistCard the Slint only uses
// on surfaces the POC does not have.)
//
// 200x246: 200px surface-card frame + 190px circle (gradient + user glyph
// until the artwork resolves), hover scrim 0.55 CLIPPED TO THE CIRCLE,
// overlay row at y=113 (follow? / play / more), pin badge top-right of
// the frame, then the meta block (centered name, optional subtitle).
//
// item contract: { id, title, subtitle, following | isFavorite } plus the
// scalar artSource / artworkUrl / isPinned props (the AlbumCard pattern).
// The follow flag arrives under TWO names because the producers spell it
// differently and both mean the same row state: `following` (search_qt
// ArtistRow, derived from fav_cache) and `isFavorite` (home_qt HomeCard /
// library_qt FeedItem — for an artist, "in the library" IS "followed", they
// are the same `favorite/create?artist_ids=` write).
//
// Arms:
//  - followMode: "toggle" (default, = ArtistGridCard.slint's own default) |
//    "none" — the overlay follow button + the menu Follow row. The Slint
//    reference passes "none" from exactly two places: the label page's
//    artist carousel (LabelPageView.slint:524) and the Library artist grids
//    (FavoritesView.slint:1831/1865). Everything else shows it.
//  - followKind: "artist" (default) | "label" — ArtistGridCard.slint's
//    `follow-kind`, the kind the follow write is sent as.
//  - subtitle: meta switches to 1-line name + muted subtitle (the Slint
//    "Similar to…"/search arm); empty = wrap-2 name.
//
// --- Menu inventory vs discover/ArtistGridCard.slint -------------------
//   Open artist · Play · Follow/Following (show-follow) · Not interested
// All four are live; "Not interested" rides `hasNotInterestedSeam` (default
// on) and writes the reco-dismissal store through QbzBlacklist.
// ⋯ and a right press on the portrait or the name open the same menu.

import QtQuick
import "../kiosk"
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    property var item: ({})
    // Host-resolved artwork path (the AlbumCard artSource pattern).
    property string artSource: ""
    // Remote cover URL for the pin payload ("" when the host has none).
    property string artworkUrl: ""
    // Pinned state (AlbumCard pattern: scalar prop, optimistic flip on
    // click — the model re-publish re-creates the delegate).
    property bool isPinned: false
    property string followMode: "toggle"
    property string followKind: "artist"

    // --- "Not interested" (the reco-scoped dismissal) --------------------
    // `dismissArtist(id, name, imageUrl)` writes the reco-dismissal store (NOT
    // the artist blacklist). Kept as a property, not inlined, so a host whose
    // `item.id` is not a Qobuz artist id can turn it off.
    //
    // The card now LEAVES THE RAIL ON THE CLICK: `blacklist_qt::dismiss_artist`
    // calls `recommendations_qt::apply_dismissal`, which drops it from the
    // cached rails and republishes. What this comment used to promise — "the
    // card leaves the rails on the next publish" — never happened, because
    // nothing read the dismissal store on publish either; the artist came
    // straight back. The ids are part of the reco exclusion set now.
    //
    // OWNER RULING (2026-08-16): the live drop is for the RECOMMENDATIONS
    // rails and nothing else. "This is not a blacklist or a general hide,
    // only for recommendations." This card is also mounted by Search, Label,
    // Artist Scene, Home and the Library grid; dismissing from one of those
    // persists and takes effect in Recommendations, but the card you clicked
    // stays where it is — deliberately. Do NOT "finish" this by making the
    // artist vanish from Search or Library; hiding an artist app-wide is the
    // BLACKLIST, a different store and a different row.
    //
    // `hasFollowSeam` is GONE, and the header comment it carried ("There is
    // NO artist-follow invokable on the Qt bridge") was simply wrong:
    // `library_bridge.rs` declares `library_toggle_favorite(kind, id)`,
    // `library_qt::toggle_favorite` routes "artist" (and "label") straight to
    // `add_favorite` / `remove_favorite`, and ArtistView's own header button
    // has been calling it all along. The constant made the follow affordance
    // disappear from EVERY artist card in the app.
    property bool hasNotInterestedSeam: true

    color: "transparent"

    QbzTheme { id: theme }

    // ArtistGridCard.slint:89 — `show-follow: follow-mode != "none"`.
    readonly property bool showFollow: followMode !== "none"
    readonly property bool overlayOn: agArea.containsMouse || pinArea.containsMouse
        || agFollow.hovered || agPlay.hovered || agMore.hovered

    implicitWidth: 200
    implicitHeight: 246

    // --- Follow state (a real QML property, never `item.following`) ------
    // Same reason as rows/TrackRow.qml's `favorite`: `item` is a plain JS
    // object, so mutating a field on it fires no notifier and re-evaluates no
    // binding — and `item: modelData` is a COPY, so the write does not reach
    // the model either (both measured under qml6 6.11.1). The seed reads both
    // spellings the producers publish (see the header note).
    property bool following: root.item.following === true
        || root.item.isFavorite === true
    onItemChanged: root.following = Qt.binding(function () {
        return root.item.following === true || root.item.isFavorite === true
    })

    function toggleFollow() {
        root.following = !root.following
        QbzLibrary.libraryToggleFavorite(root.followKind, root.item.id)
    }

    function openMenu(anchor, x, y) {
        agMenuLoader.active = true
        agMenuLoader.item.openAtCursor(anchor, x, y)
    }
    function releaseForReuse() {
        if (agMenuLoader.item)
            agMenuLoader.item.close()
        agMenuLoader.active = false
    }

    Connections {
        target: QbzLibrary
        // Pin fan-out — the AlbumCard contract, artist key (see AlbumCard.qml:
        // the store has no change-notify, so this signal is what keeps every
        // OTHER card showing this artist honest without a republish).
        function onPinChanged(key, value) {
            var aid = (root.item && root.item.id !== undefined) ? root.item.id : ""
            if (aid !== "" && key === "artist:" + aid)
                root.isPinned = value
        }
        // Follow fan-out + rollback, the favourite twin of the above. The key
        // is `{followKind}:{id}` so a label-follow card settles on its own
        // key rather than on an artist one that happens to share the number.
        function onLibraryFavoriteChanged(key, value) {
            var aid = (root.item && root.item.id !== undefined) ? root.item.id : ""
            if (aid !== "" && key === root.followKind + ":" + aid)
                root.following = value
        }
    }

    Column {
        spacing: 0
        Rectangle {
            width: 200
            height: 200
            radius: theme.radiusSm
            color: theme.surfaceCard

            // 190px round portrait, centered (5px surround = the
            // album-tile frame). All overlay content lives inside so it
            // clips to the circle.
            Rectangle {
                x: 5
                y: 5
                width: 190
                height: 190
                radius: 95
                // No clip. It never made the circle — QML's clip is a
                // rectangular scissor and ignores `radius`
                // (qquickclipnode.cpp:13-22); the round shape comes from each
                // child's own `radius` / RoundedImage mask. Geometrically
                // nothing leaves the 190x190 box.
                gradient: Gradient {
                    orientation: Gradient.Horizontal
                    GradientStop { position: 0.0; color: theme.surfaceElevated }
                    GradientStop { position: 1.0; color: theme.surfaceCard }
                }
                // Placeholder glyph until the artwork resolves on top.
                QbzIcon {
                    name: "user"
                    width: 54
                    height: 54
                    anchors.centerIn: parent
                    tintName: "muted"
                }
                Loader {
                    anchors.fill: parent
                    sourceComponent: root.kioskHost ? kioskCover : desktopCover
                    Component {
                        id: kioskCover
                        KioskArtwork { anchors.fill: parent; source: root.artSource; radius: 95 }
                    }
                    Component {
                        id: desktopCover
                        RoundedImage { anchors.fill: parent; source: root.artSource; radius: 95 }
                    }
                }
                // Hover scrim (clipped to the circle by the parent's clip).
                Rectangle {
                    anchors.fill: parent
                    radius: 95
                    color: "#000000"
                    opacity: root.overlayOn ? 0.55 : 0.0
                    Behavior on opacity { NumberAnimation { duration: 150 } }
                }
                // Card-open + hover detector (before the buttons so they
                // win the pointer).
                MouseArea {
                    id: agArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: function (mouse) {
                        if (mouse.button === Qt.RightButton)
                            root.openMenu(agArea, mouse.x, mouse.y)
                        else
                            QbzArtist.openArtist(root.item.id)
                    }
                }
                // Hover overlay — follow? / play / more (y=113).
                CardOverlayRow {
                    height: root.kioskHost ? 64 : 44
                    y: root.kioskHost ? 104 : 113
                    width: parent.width
                    shown: root.kioskHost || root.overlayOn
                    CardOverlayButton {
                        width: root.kioskHost ? (primary ? 64 : 44) : (primary ? 44 : 36)
                        height: width
                        id: agFollow
                        visible: root.showFollow
                        name: root.following ? "check" : "user-plus"
                        active: root.following
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: root.toggleFollow()
                    }
                    CardOverlayButton {
                        width: root.kioskHost ? (primary ? 64 : 44) : (primary ? 44 : 36)
                        height: width
                        id: agPlay
                        name: "play-fill"
                        primary: true
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: QbzPlayer.playArtistCard(root.item.id)
                    }
                    CardOverlayButton {
                        width: root.kioskHost ? (primary ? 64 : 44) : (primary ? 44 : 36)
                        height: width
                        id: agMore
                        name: "ellipsis"
                        anchors.verticalCenter: parent.verticalCenter
                        onClicked: function (mouse) { root.openMenu(agMore, mouse.x, mouse.y) }
                    }
                }
                Loader {
                    id: agMenuLoader
                    active: false
                    sourceComponent: CardMenu {
                        kioskHost: root.kioskHost
                        menuWidth: 196
                        entries: root.menuModel()
                        onPicked: function (a) { root.menuAction(a) }
                    }
                }
            }
            // Pin badge — top-right of the FRAME (outside the circle clip),
            // opacity follows overlay-on (the AlbumCard convention).
            Rectangle {
                x: parent.width - width - 8
                y: 8
                width: root.kioskHost ? 44 : 26
                height: root.kioskHost ? 44 : 26
                radius: 13
                color: pinArea.containsMouse ? "#cc000000" : "#99000000"
                opacity: root.kioskHost || root.overlayOn ? 1.0 : 0.0
                Behavior on opacity { NumberAnimation { duration: 150 } }
                QbzIcon {
                    name: root.isPinned ? "pin-filled" : "pin"
                    width: 14
                    height: 14
                    anchors.centerIn: parent
                    // On a #99000000/#cc000000 scrim — dark under every theme.
                    tintName: root.isPinned ? "accent" : "white"
                }
                MouseArea {
                    id: pinArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        root.isPinned = !root.isPinned
                        QbzLibrary.togglePin("artist", root.item.id, root.item.title,
                            root.item.subtitle || "", root.artworkUrl)
                    }
                }
            }
        }
        Item { width: 1; height: 6 }
        // Meta: centered name; subtitle arm switches to 1-line + muted row.
        Column {
            width: 200
            height: 40
            spacing: 2
            Text {
                width: parent.width
                height: root.item.subtitle ? 20 : 40
                text: root.item.title || ""
                color: agNameArea.containsMouse ? theme.accent : theme.textPrimary
                font.pixelSize: theme.fontBody - 2
                font.weight: theme.weightMedium
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                wrapMode: root.item.subtitle ? Text.NoWrap : Text.WordWrap
                maximumLineCount: 2
                elide: Text.ElideRight
                MouseArea {
                    id: agNameArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: function (mouse) {
                        if (mouse.button === Qt.RightButton)
                            root.openMenu(agNameArea, mouse.x, mouse.y)
                        else
                            QbzArtist.openArtist(root.item.id)
                    }
                }
            }
            Text {
                visible: !!root.item.subtitle
                width: parent.width
                height: 16
                text: root.item.subtitle || ""
                color: theme.textMuted
                font.pixelSize: theme.fontLink - 1
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
        }
    }

    // ArtistGridCard.slint's artist-menu, in its order.
    function menuModel() {
        var t = QbzSession.tr
        var r = QbzSession.trRev
        var m = [
            { "label": t("Open artist", r), "icon": "user", "action": "open" },
            { "label": t("Play", r), "icon": "play-fill", "action": "play" },
        ]
        if (root.showFollow)
            m.push({ "label": root.following ? t("Following", r) : t("Follow", r),
                     "icon": root.following ? "check" : "user-plus", "action": "follow" })
        // QoL round: the Discography Builder, reachable from the card — the
        // same seam ArtistView's ⋯ overflow uses (`QbzDisco.open` takes the
        // artist id, no open view required).
        m.push({ "label": t("Create Artist Collection", r), "icon": "library-big", "action": "disco" })
        // Reco-scoped dismissal (NOT the blacklist) — the artist leaves the
        // Recommendations rails only.
        if (root.hasNotInterestedSeam)
            m.push({ "label": t("Not interested", r), "icon": "thumbs-down", "action": "not-interested" })
        return m
    }

    function menuAction(a) {
        if (a === "open") QbzArtist.openArtist(root.item.id)
        else if (a === "play") QbzPlayer.playArtistCard(root.item.id)
        else if (a === "follow") root.toggleFollow()
        else if (a === "disco") QbzDisco.open(root.item.id)
        // THREE args: the store persists (id, name, image_url) and only
        // backfills a field that is EMPTY, so a blank first write can never be
        // repaired later. `artworkUrl` first because that is the host-
        // normalized REMOTE url (some producers spell the key `imageUrl`, e.g.
        // LibraryView) — `item.artUrl` is the HomeCard fallback, and never
        // `artSource`, which is a local file:// cache path.
        else if (a === "not-interested")
            QbzBlacklist.dismissArtist(root.item.id,
                root.item.name || root.item.title || "",
                root.artworkUrl !== "" ? root.artworkUrl : (root.item.artUrl || ""))
    }
}
