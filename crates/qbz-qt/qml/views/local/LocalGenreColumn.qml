// One column of the Local Library Genres browser. Selection is controlled by
// the host: plain click replaces, Ctrl (or Command on macOS) toggles.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"

Rectangle {
    id: root

    property string title: ""
    property string allLabel: ""
    property string query: ""
    property string editQuery: query
    // The last column is a direct title lookup over the already-projected
    // logical albums, so it can publish sooner than the upstream facets whose
    // edits rebuild the options to their right.
    property int debounceMs: 90
    property var options: []
    property var selected: ({})
    signal queryEdited(string value)
    signal toggled(string key, int modifiers)

    onQueryChanged: {
        if (editQuery !== query) editQuery = query
    }

    Timer {
        id: queryDebounce
        interval: root.debounceMs
        repeat: false
        onTriggered: root.queryEdited(root.editQuery)
    }

    QbzTheme { id: theme }

    // Keep the browser legible over album artwork without turning Ambient
    // back into an opaque black panel. The 50% neutral base lets the palette
    // through while the frost edge keeps the three chained columns distinct.
    radius: theme.radiusSm
    clip: true
    color: theme.ambientOn ? theme.surfaceMainA50 : theme.surfaceMain
    border.width: 1
    border.color: theme.ambientOn ? theme.frostBorder : theme.borderSubtle

    Text {
        id: heading
        x: 10
        y: 7
        text: root.title
        color: theme.textSecondary
        font.pixelSize: theme.fontLegal
        font.weight: theme.weightSemibold
    }

    QbzLineEdit {
        id: search
        x: 8
        y: 27
        width: parent.width - 16
        height: 30
        searchMode: true
        sm: true
        text: root.editQuery
        placeholder: QbzSession.tr("Search", QbzSession.trRev)
        onEdited: function(value) {
            root.editQuery = value
            if (value === "") {
                queryDebounce.stop()
                root.queryEdited(value)
            } else {
                queryDebounce.restart()
            }
        }
    }

    ListView {
        id: list
        x: 1
        y: 64
        width: parent.width - 2
        height: parent.height - y - 1
        clip: true
        reuseItems: true
        boundsBehavior: Flickable.StopAtBounds
        model: [{ "key": "", "label": root.allLabel }].concat(root.options || [])

        delegate: Rectangle {
            required property var modelData
            width: list.width
            height: 28
            readonly property bool active: modelData.key === ""
                ? Object.keys(root.selected || {}).length === 0
                : root.selected[modelData.key] === true
            color: active
                ? (theme.ambientOn ? theme.surfaceElevatedA50 : theme.alphaTier(10))
                : area.containsMouse
                    ? (theme.ambientOn ? theme.surfaceElevatedA50 : theme.surfaceHover)
                    : "transparent"

            Text {
                x: 10
                width: parent.width - 32
                height: parent.height
                verticalAlignment: Text.AlignVCenter
                text: modelData.label || ""
                color: parent.active ? theme.textPrimary : theme.textSecondary
                font.pixelSize: 12
                font.weight: parent.active ? theme.weightSemibold : theme.weightRegular
                elide: Text.ElideRight
            }
            QbzIcon {
                visible: parent.active
                name: "check"
                width: 12
                height: 12
                anchors.right: parent.right
                anchors.rightMargin: 9
                anchors.verticalCenter: parent.verticalCenter
                tintName: "accent"
            }
            MouseArea {
                id: area
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: function(mouse) { root.toggled(modelData.key, mouse.modifiers) }
            }
        }
    }

    QbzScrollBar {
        target: list
        anchors.right: parent.right
        anchors.top: list.top
        anchors.bottom: list.bottom
    }
}
