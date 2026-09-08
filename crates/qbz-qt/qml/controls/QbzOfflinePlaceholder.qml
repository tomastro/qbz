// QbzOfflinePlaceholder — the offline gate (offline/OfflinePlaceholder
// .slint), consolidated in phase 22 from the TWO verbatim copies
// (HomeView + LibraryView). Arms: `induced` (offlineMode === 2) switches
// the body copy; `showSettingsAction` mounts the induced-only "Open
// Settings" button (POC-NOTE: navigates via QbzShell.navigateTo —
// the Slint opens Settings the same way).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Column {
    property bool induced: QbzSession.offlineMode === 2
    property bool showSettingsAction: false
    signal settingsClicked()

    QbzTheme { id: theme }

    spacing: 0
    QbzIcon {
        name: "cloud-off"
        width: 56
        height: 56
        anchors.horizontalCenter: parent.horizontalCenter
        tintName: "muted"
    }
    Item { width: 1; height: 18 }
    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: QbzSession.tr("You're offline", QbzSession.trRev)
        color: theme.textPrimary
        font.pixelSize: theme.fontHeading
        font.weight: theme.weightSemibold
    }
    Item { width: 1; height: 8 }
    Text {
        width: 420
        // THREE cases, not two. The third is a session that was started
        // WITHOUT a Qobuz account at all ("Start offline" on the login
        // screen): that user has working internet, and telling them "no
        // internet connection" is simply false — measured on a fresh profile
        // 2026-08-22, where the log read `connectivity Up, offline_session
        // true` while this line claimed the network was down.
        text: induced
            ? QbzSession.tr("Offline mode is enabled. Disable it in Settings to use Qobuz.", QbzSession.trRev)
            : QbzSession.offlineSession
                ? QbzSession.tr("You're using QBZ without a Qobuz account. Your local library and downloads keep working.", QbzSession.trRev)
                : QbzSession.tr("No internet connection. Your local library and downloads keep working.", QbzSession.trRev)
        color: theme.textSecondary
        font.pixelSize: theme.fontBody
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
    }
    Item { visible: showSettingsAction && induced; width: 1; height: 18 }
    SettingsButton {
        visible: showSettingsAction && induced
        anchors.horizontalCenter: parent.horizontalCenter
        text: QbzSession.tr("Open Settings", QbzSession.trRev)
        onClicked: parent.settingsClicked()
    }
}
