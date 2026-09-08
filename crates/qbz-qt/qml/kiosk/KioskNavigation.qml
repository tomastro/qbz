// View-owned opaque state on the single QbzShell history. No private stack.
import QtQuick
import com.blitzfc.qbz

Item {
    id: root
    property string route: ""
    property var snapshot: ({})
    readonly property string stateJson: JSON.stringify(snapshot)
    property bool ready: false
    property bool restoring: false
    signal restore(var saved)
    width: 0
    height: 0
    visible: false

    function recordTab(tab) {
        if (ready && !restoring && QbzShell.currentView === route)
            QbzShell.recordKioskTab(route, String(tab), stateJson)
    }
    function report() {
        if (ready && !restoring && QbzShell.currentView === route)
            QbzShell.reportNavState(route, stateJson)
    }
    function restoreState() {
        if (QbzShell.currentView !== route || QbzShell.restoreStateScope !== route)
            return
        var saved
        try { saved = JSON.parse(QbzShell.stateRestore) }
        catch (e) { return }
        restoring = true
        restore(saved)
        QbzShell.restoreStateScope = ""
        restoring = false
    }
    onStateJsonChanged: report()
    Component.onCompleted: {
        if (QbzShell.restoreStateScope === route) {
            restoreState()
        } else {
            // A live Desktop -> Kiosk switch carries the current explicit tab.
            // Fresh starts have no live snapshot and keep the Albums fallback.
            var live = QbzShell.navigationState(route)
            if (live !== "") {
                try { restoring = true; restore(JSON.parse(live)) }
                catch (e) { /* An invalid optional snapshot keeps fresh defaults. */ }
                finally { restoring = false }
            }
        }
        Qt.callLater(function() { root.ready = true; root.report() })
    }
    Connections {
        target: QbzShell
        function onStateRestoreChanged() { root.restoreState() }
        function onCurrentViewChanged() { root.restoreState() }
    }
}
