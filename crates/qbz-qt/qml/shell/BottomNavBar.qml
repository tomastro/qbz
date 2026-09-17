// BottomNavBar — Apple Music style bottom navigation bar for mobile / portrait mode.
// Provides 5 primary tabs: Home, Browse, Radio, Library, Search.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root

    property real bottomSafeInset: 0
    readonly property real barHeight: 50

    height: barHeight + bottomSafeInset
    color: theme.ambientOn ? theme.surfaceCardA50 : theme.surfaceCard
    z: 900

    QbzTheme { id: theme }

    // Top hairline border
    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: 1
        color: theme.borderSubtle
        opacity: 0.6
    }

    // Tabs definition
    readonly property var tabs: [
        {
            "id": "home",
            "label": QbzSession.tr("Home", QbzSession.trRev),
            "icon": "house",
            "route": "home"
        },
        {
            "id": "discoverbrowse",
            "label": QbzSession.tr("Browse", QbzSession.trRev),
            "icon": "layout-grid",
            "route": "discoverbrowse"
        },
        {
            "id": "library",
            "label": QbzSession.tr("Library", QbzSession.trRev),
            "icon": "library",
            "route": "library"
        },
        {
            "id": "search",
            "label": QbzSession.tr("Search", QbzSession.trRev),
            "icon": "search",
            "route": "search"
        }
    ]

    function isTabActive(route) {
        if (route === "home") {
            return QbzShell.currentView === "home" || QbzShell.currentView === ""
        }
        return QbzShell.currentView === route
    }

    Row {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        height: root.barHeight

        Repeater {
            model: root.tabs

            Item {
                id: tabItem
                width: parent.width / root.tabs.length
                height: root.barHeight

                readonly property bool active: root.isTabActive(modelData.route)

                Column {
                    anchors.centerIn: parent
                    spacing: 3

                    Item {
                        width: 22
                        height: 22
                        anchors.horizontalCenter: parent.horizontalCenter

                        QbzIcon {
                            anchors.centerIn: parent
                            name: modelData.icon
                            width: 20
                            height: 20
                            tintName: tabItem.active ? "accent" : "textMuted"
                        }
                    }

                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: modelData.label
                        color: tabItem.active ? theme.accent : theme.textMuted
                        font.pixelSize: 10
                        font.weight: tabItem.active ? theme.weightSemibold : theme.weightRegular
                        elide: Text.ElideRight
                    }
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        if (modelData.route === "discoverbrowse") {
                            QbzHome.openDiscoverBrowse("/discover/newReleases", QbzSession.tr("New Releases", QbzSession.trRev))
                        } else {
                            QbzShell.navigateTo(modelData.route)
                        }
                    }
                }
            }
        }
    }
}