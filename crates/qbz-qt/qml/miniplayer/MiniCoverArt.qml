// MiniCoverArt — the mini's cover tile (2026-08-03 miniplayer/tray contract
// A-28, §4.2.3), port of `CoverArt` at
// `crates/qbz-ui/ui/miniplayer/MiniBits.slint:28-44`. The other half of the
// MiniBits split; see MiniExplicitBadge.qml.
//
// THE PLACEHOLDER IS THE BACKGROUND SHOWING THROUGH AN EMPTY IMAGE — no icon,
// no glyph, no skeleton (MiniBits.slint:25-27). RoundedImage renders nothing
// while `source` is empty, so the tinted fill and its 1 px border are the
// empty state, exactly as the reference's bordered empty-art box is.
//
// `radius` is Rectangle's OWN property, defaulted to 7 here rather than
// redeclared: Slint's `in property <length> radius: 7px` and a QML
// `property real radius` on a Rectangle root are the same knob, and shadowing
// it would be a property-override. The default is never actually used — the
// two call sites pass 8 (compact) and 10 (artwork).
//
// RoundedImage's DEFAULT arm is the one that matters here (§7-M3): Image +
// layer + a MultiEffect masked by a rounded Rectangle, whose FBOs are pure
// functions of (width, height, radius), so a static cover costs zero CPU raster
// and zero texture upload per frame. The mini's cover does not change between
// frames — it is the clearest "static content, cache it" case in the app. Do
// not reach for the Canvas arm.

import QtQuick
import "../theme"

Rectangle {
    id: root

    QbzTheme { id: theme }

    /// A file:// path from the artwork pipeline (QbzPlayer.npArtworkPath), not
    /// an image object — the Qt component takes URL strings where the Slint one
    /// takes `image`.
    property string source: ""

    radius: 7
    antialiasing: true
    color: theme.alphaTier(8)
    border.width: 1
    border.color: theme.alphaTier(12)
    clip: true

    RoundedImage {
        anchors.fill: parent
        source: root.source
        radius: root.radius
    }
}
