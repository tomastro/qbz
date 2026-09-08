// Centered muted note — the empty / no-match / nothing-selected body every
// tab in LocalLibraryView.slint uses (a centered VerticalLayout with one
// text-muted Typography.body line).

import QtQuick
import "../../theme"

Text {
    QbzTheme { id: theme }
    anchors.centerIn: parent
    color: theme.textMuted
    font.pixelSize: theme.fontBody
    horizontalAlignment: Text.AlignHCenter
}
