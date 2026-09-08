// Kiosk Discover: recycled vertical shelves and recycled horizontal cards.
import QtQuick
import com.blitzfc.qbz
import "../theme"
import "../controls"

Rectangle {
    id: root
    color: "transparent"
    property string activeTab: "home"
    property var cursors: ({})
    property var shelfPositions: ({})
    function saveShelf(key, value) { var next = Object.assign({}, shelfPositions); next[key] = value; shelfPositions = next }
    readonly property var tabIds: ["home", "editorPicks", "forYou"]
    property var tabNavigationRequest: ({})
    onTabNavigationRequestChanged: if (tabNavigationRequest && tabNavigationRequest.tab !== undefined) activateTab(tabNavigationRequest.tab)
    function activateTab(tab) { selectTab(tab) }
    function selectTab(tab) {
        if (tabIds.indexOf(tab) < 0 || tab === activeTab) return
        navigation.recordTab(tab)
        activeTab = tab
    }
    QbzTheme { id: theme }
    function parse(value) { try { var result = JSON.parse(value); return Array.isArray(result) ? result : [] } catch(e) { return [] } }
    readonly property var sections: {
        var data = parse(activeTab === "editorPicks" ? QbzHome.editorSectionsJson
            : activeTab === "forYou" ? QbzHome.forYouSectionsJson : QbzHome.homeSectionsJson)
        return data.filter(function(s) { return s.kind === "album" && (s.items || []).length > 0 })
    }
    readonly property bool contentFocus: QbzKioskNav.navActive && QbzKioskNav.zone === "content"
    readonly property int shelfIndex: QbzKioskNav.index - 3
    function publishNav() { QbzKioskNav.publishNav(3, 1, 3 + sections.length, true) }
    onSectionsChanged: Qt.callLater(publishNav)
    Component.onCompleted: publishNav()
    KioskNavigation {
        id: navigation
        route: "home"
        snapshot: ({ activeTab: root.activeTab, cursors: root.cursors, shelfPositions: root.shelfPositions })
        onRestore: function(saved) {
            if (root.tabIds.indexOf(saved.activeTab) >= 0) root.activeTab = saved.activeTab
            root.cursors = saved.cursors || ({})
            root.shelfPositions = saved.shelfPositions || ({})
        }
    }
    Connections {
        target: QbzKioskNav
        function onIndexChanged() {
            if (root.contentFocus && root.shelfIndex >= 0)
                shelves.positionViewAtIndex(root.shelfIndex, ListView.Contain)
        }
        function onHmoveChanged() {
            if (!root.contentFocus || root.shelfIndex < 0 || root.shelfIndex >= root.sections.length || !QbzKioskNav.hmove) return
            var key = root.activeTab + ":" + root.shelfIndex
            var next = Math.max(0, Math.min((root.cursors[key] || 0) + QbzKioskNav.takeHmove(), root.sections[root.shelfIndex].items.length - 1))
            var positions = Object.assign({}, root.cursors); positions[key] = next; root.cursors = positions
        }
        function onActivateSeqChanged() {
            if (!root.contentFocus) return
            if (QbzKioskNav.index < 3) { root.selectTab(root.tabIds[QbzKioskNav.index]); return }
            if (root.shelfIndex < 0 || root.shelfIndex >= root.sections.length) return
            var row = root.sections[root.shelfIndex].items[root.cursors[root.activeTab + ":" + root.shelfIndex] || 0]
            if (row) QbzAlbum.openAlbum(row.id)
        }
    }
    Row {
        id: tabs
        x: 16; height: 64; spacing: 8
        Repeater {
            model: root.tabIds
            delegate: Rectangle {
                required property string modelData
                required property int index
                width: Math.max(64, Math.min((root.width - 48) / 3, label.implicitWidth + 24))
                height: 64; radius: theme.radiusSm
                color: root.activeTab === modelData ? theme.surfaceElevated : "transparent"
                border.width: root.contentFocus && QbzKioskNav.index === index ? 2 : 0
                border.color: theme.accent
                Text {
                    id: label; anchors.centerIn: parent
                    text: QbzSession.tr(index === 0 ? "Home" : index === 1 ? "Editor's Picks" : "For You", QbzSession.trRev)
                    color: root.activeTab === modelData ? theme.textPrimary : theme.textSecondary
                    font.pixelSize: 16
                }
                MouseArea { anchors.fill: parent; onClicked: root.selectTab(modelData) }
            }
        }
    }
    ListView {
        id: shelves
        anchors.top: tabs.bottom; anchors.bottom: parent.bottom
        anchors.left: parent.left; anchors.right: parent.right
        leftMargin: 16; rightMargin: 16; topMargin: 12; bottomMargin: 16
        clip: true; spacing: 20; cacheBuffer: 0; reuseItems: true
        model: root.sections
        boundsBehavior: Flickable.StopAtBounds
        delegate: Column {
            required property var modelData
            required property int index
            width: shelves.width - 32; spacing: 12
            Text {
                width: parent.width; height: 28
                text: modelData.title || ""; color: theme.textPrimary
                font.pixelSize: 18; font.weight: theme.weightSemibold; elide: Text.ElideRight
            }
            KioskShelf {
                width: parent.width
                rows: modelData.items || []
                navFocused: root.contentFocus && root.shelfIndex === index
                cursor: root.cursors[root.activeTab + ":" + index] || 0
                restoreX: root.shelfPositions[root.activeTab + ":" + index] || 0
                onScrollSettled: function(x) { root.saveShelf(root.activeTab + ":" + index, x) }
                onOpen: function(id) { QbzAlbum.openAlbum(id) }
            }
        }
    }
    ScrollMemory { target: shelves; scope: "home:" + root.activeTab }
    KioskSkeleton {
        anchors.fill: shelves
        kind: "grid"
        loading: QbzHome.homeLoading && root.sections.length === 0
        error: QbzHome.homeError
        empty: !QbzHome.homeLoading && root.sections.length === 0
    }
}
