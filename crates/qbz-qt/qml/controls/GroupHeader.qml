// Settings group header (AppearanceSettings.slint GroupHeader) — extracted
// from SettingsView.qml in phase 19: 11px muted uppercase, 1.5px tracking,
// semibold.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Text {
    // Explicit density opt-in; desktop geometry remains the default.
    property bool kioskHost: false

    QbzTheme { id: theme }

    color: theme.textMuted
    font.pixelSize: kioskHost ? 13.2 : 11
    font.letterSpacing: 1.5
    font.weight: theme.weightSemibold
}
