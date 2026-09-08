// PmLocalBadge — the 22 px availability disc on a grid card's artwork
// (PlaylistManagerView.slint:261-287). Three mutually exclusive glyphs:
//
//   isLocal            -> hard-drive, tint `accent` when the local playlist is
//                         offline-only, else `text-secondary`
//   status all_local   -> wifi,           literal #22c55e
//   status some_local  -> cloud-download, literal #f59e0b
//
// INTERFACE NOTE (contract §2.5). The contract declares `status` only;
// `isLocal` / `offlineOnly` are ADDED because the reference badge takes both
// (`is-local-pl` :265, `offline-only` :266) and they select the glyph AND its
// tint — a first-class local playlist publishes `localStatus: ""` (§4.3), so
// with `status` alone the badge would be invisible on every local row. Reported
// as an interface addition.
//
// COLOUR TRAP (contract §0.1). Slint's 8-digit literals are #RRGGBBAA, Qt's are
// #AARRGGBB: the reference plate `#00000099` (:272) is black at 60 % alpha and
// is written `#99000000` here. Copied verbatim it would have been a nearly
// opaque near-black-blue.
//
// The two brand glyph colours are NOT theme tokens and QbzIcon takes a tint
// NAME, not a hex — they ride the `green` / `orange` one-off tint dirs the
// asset pass adds (contract §7.3), following the `amber` precedent.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Rectangle {
    id: root

    /// `"no" | "some_local" | "all_local"`; `""` on a first-class local row.
    property string status: "no"
    /// True for a `local:<uuid>` playlist (the hard-drive arm).
    property bool isLocal: false
    /// Locals only: an offline-only playlist tints its marker `accent`.
    property bool offlineOnly: false

    visible: root.isLocal || root.status === "all_local"
        || root.status === "some_local"

    width: 22
    height: 22
    radius: 11
    // Slint #00000099 -> Qt #AARRGGBB. See the header.
    color: "#99000000"

    QbzIcon {
        anchors.centerIn: parent
        width: 13
        height: 13
        name: root.isLocal ? "hard-drive"
            : (root.status === "all_local" ? "wifi" : "cloud-download")
        tintName: root.isLocal
            ? (root.offlineOnly ? "accent" : "secondary")
            : (root.status === "all_local" ? "green" : "orange")
    }
}
