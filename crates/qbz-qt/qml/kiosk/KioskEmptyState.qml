// KioskEmptyState — the static "there is nothing here" / "this failed" panel
// of the kiosk shell.
//
// Contract §1.7: a kiosk route must let the user tell loading, empty, error and
// populated apart without looking dead. This is the second and third of those
// four; KioskSkeleton is the first, and it is what mounts this file.
//
// DELIBERATELY INERT. No icon, no artwork, no button, no animation, no timer.
//   - no artwork, because an empty state that mounts an image is a network
//     request for a state that by definition has no content (contract §5.2:
//     "un tab/card no montado no solicita imagen");
//   - no action, because the RETRY lives with whoever owns the document. This
//     panel is presentational; a host that wants a retry puts its own control
//     under it and gets the ≥64 px target from its own chrome.
//
// TEXT IS SUPPLIED, NOT INVENTED. `text` and `detail` arrive already
// translated from the host (KioskSkeleton passes an existing msgid through
// QbzSession.tr, and an error string arrives from the document that produced
// it). Nothing here hardcodes English.
//
// Sizing is for the 800x480 floor: the label wraps, is capped at four lines and
// elides, and the whole block stays inside the item, so a long German or
// Russian string cannot push itself off a Pi panel.

import QtQuick
import "../theme"

Item {
    id: root

    /// The headline. Already translated.
    property string text: ""
    /// An optional second line — a document's own error detail, a hint. Already
    /// translated. Empty hides it.
    property string detail: ""
    /// Error tone: the headline takes the secondary text colour instead of the
    /// muted one, so a failure reads louder than an emptiness without
    /// introducing a red the kiosk theme does not define.
    property bool isError: false
    /// Horizontal breathing room. 24 keeps the block clear of the NavRail at
    /// 800 px wide.
    property real pad: 24

    QbzTheme { id: theme }

    implicitHeight: block.implicitHeight

    Column {
        id: block
        anchors.centerIn: parent
        width: Math.max(0, root.width - 2 * root.pad)
        spacing: 8

        Text {
            width: block.width
            text: root.text
            color: root.isError ? theme.textSecondary : theme.textMuted
            font.pixelSize: 16
            font.weight: theme.weightMedium
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            maximumLineCount: 3
            elide: Text.ElideRight
        }

        Text {
            width: block.width
            visible: root.detail !== ""
            text: root.detail
            color: theme.textMuted
            font.pixelSize: 13
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            maximumLineCount: 4
            elide: Text.ElideRight
        }
    }
}
