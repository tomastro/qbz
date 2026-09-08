// CardOverlayRow — the hover action row every card family shows over its
// artwork (favorite / play / more, plus follow on artists).
//
// WHY THIS EXISTS: the four cards had each hand-rolled this row and all four
// were misaligned, each in its own way. They centred the group with a manual
// spacer Item computed as `(cardWidth - 44 - 2*36 - 2*12) / 2` — the width
// left over once you account for the primary disc, two ghost discs and the
// two gaps between them:
//
//   - AlbumCard set `spacing: 12` AND kept the spacer, so the spacer itself
//     got a 12px gap after it and the group sat 12px RIGHT of centre.
//   - ArtistCard, PlaylistCard and TrackCard left `spacing` unset, so the
//     gaps the formula had already subtracted never materialised: the discs
//     touched each other and the group sat 12px LEFT of centre.
//   - ArtistCard additionally measured against 190px while its art is 200.
//
// The arithmetic was never needed. A Row centred on its parent computes its
// own width from its children, whatever they are — which also means a card
// that hides one button (artists without the follow action) stays centred for
// free, where the old formula needed a conditional term to keep up.
//
// Consumers give the row a `y` and the card's width, mount their buttons as
// children, and drive `shown` from the card's hover state.

import QtQuick

Item {
    id: root

    /// Card hover state — the row cross-fades with the scrim.
    property bool shown: false
    /// Gap between the discs. 12px in every card family.
    property int spacing: 12

    default property alias buttons: row.data

    // The discs are 44px (primary) and 36px (ghost); the row is as tall as
    // the tallest so children can centre vertically against it.
    height: 44
    opacity: shown ? 1.0 : 0.0
    visible: opacity > 0
    Behavior on opacity { NumberAnimation { duration: 150 } }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: root.spacing
    }
}
