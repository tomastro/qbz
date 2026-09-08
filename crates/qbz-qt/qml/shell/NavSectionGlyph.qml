// The leading glyph of a nav SECTION, shared by the three hosts that render
// one: Sidebar.qml's NavRow, HeaderBar.qml's NavTab and its CompactNavBtn.
// The catalog (NavFlyout.sections) carries both an `icon` (baked glyph name)
// and an `iconPath` (a user-supplied image, "" when there is none), and the
// branch between them is Slint's SidebarNavRow `raw-icon` arm:
// a custom icon is rendered RAW so its own colours show, every other section
// keeps the theme-tinted glyph (Sidebar.slint:559-562, :601-612).
//
// It exists as its own component because the alternative is the identical
// two-way branch copy-pasted at three call sites (TRACK-RULES §5).

import QtQuick
import "../theme"

Item {
    id: glyph

    // A section object from NavFlyout.sections.
    property var section: null
    // Square edge, in px. Sites that need an asymmetric width (the header tab
    // collapses its icon to 0 under the 1140px breakpoint) set `width` too.
    property real size: 16
    // QbzIcon tint for the baked-glyph arm; ignored by the raw arm.
    property string tintName: "secondary"

    implicitWidth: size
    implicitHeight: size

    readonly property string iconPath:
        (section && section.iconPath) ? section.iconPath : ""

    // Raw user icon — no tint, `contain` fit (Slint image-fit: contain).
    Image {
        anchors.fill: parent
        visible: glyph.iconPath !== ""
        source: glyph.iconPath
        sourceSize: Qt.size(Math.max(1, Math.round(width * 2)),
                            Math.max(1, Math.round(height * 2)))
        fillMode: Image.PreserveAspectFit
        asynchronous: true
    }

    // Default branded glyph — tinted to the host's text colour.
    QbzIcon {
        anchors.fill: parent
        visible: glyph.iconPath === ""
        // Kept empty on the raw arm so the baked glyph is never loaded twice.
        name: (glyph.section && glyph.iconPath === "") ? glyph.section.icon : ""
        tintName: glyph.tintName
    }
}
