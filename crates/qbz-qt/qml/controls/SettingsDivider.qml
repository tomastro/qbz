// Settings 1px divider (settings panel group separator) — extracted from
// SettingsView.qml in phase 19 (named SettingsDivider so it never shadows
// QtQuick.Controls.Divider in the shared qml directory).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    QbzTheme { id: theme }

    width: parent ? parent.width : 0
    height: 1
    color: theme.borderSubtle
}
