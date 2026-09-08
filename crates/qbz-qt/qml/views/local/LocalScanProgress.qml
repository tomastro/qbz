// Compact, read-only progress for the Local Library header. It combines the
// authoritative folder scanner with the derived catalog projection, but
// never reads rows from either model: only bounded counters cross into QML,
// preserving native paging and first-paint performance.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Item {
    id: root

    QbzTheme { id: theme }

    readonly property var settingsDoc: {
        try { return JSON.parse(QbzBridge.settingsJson || "{}") }
        catch (e) { return ({}) }
    }
    readonly property var localScan: settingsDoc.library || ({})
    readonly property var catalog: {
        try { return JSON.parse(QbzLocal.localCatalogProgressJson || "{}") }
        catch (e) { return ({}) }
    }
    readonly property bool folderActive: localScan.scanning === true
    readonly property bool catalogActive: catalog.active === true
    readonly property bool active: folderActive || catalogActive

    readonly property real sourceDone: folderActive
        ? Number(localScan.sourceProcessed || 0) : Number(catalog.sourceDone || 0)
    readonly property real sourceTotal: folderActive
        ? Number(localScan.sourceTotal || 0) : Number(catalog.sourceTotal || 0)
    readonly property real overallDone: folderActive
        ? Number(localScan.processed || 0) : Number(catalog.overallDone || 0)
    readonly property real overallTotal: folderActive
        ? Number(localScan.total || 0) : Number(catalog.overallTotal || 0)
    readonly property int sourceIndex: folderActive
        ? Number(localScan.sourceIndex || 0) : Number(catalog.sourceIndex || 0)
    readonly property int sourceCount: folderActive
        ? Number(localScan.sourceCount || 0) : Number(catalog.sourceCount || 0)

    function boundedFraction(done, total) {
        return total > 0 ? Math.max(0, Math.min(1, done / total)) : 0
    }
    function folderName() {
        var id = Number(localScan.currentRootId || 0)
        var folders = localScan.folders || []
        for (var i = 0; i < folders.length; i++) {
            if (Number(folders[i].id || 0) === id)
                return folders[i].displayName || folders[i].path || ""
        }
        return QbzSession.tr("Local folders", QbzSession.trRev)
    }
    function catalogSourceName() {
        var source = String(catalog.source || "")
        if (source === "local")
            return QbzSession.tr("Local folders", QbzSession.trRev)
        if (source === "offline")
            return QbzSession.tr("Offline", QbzSession.trRev)
        if (source === "plex") return "Plex"
        if (source === "jellyfin") return "Jellyfin"
        if (source === "subsonic") return "Subsonic"
        return String(catalog.sourceInstance || source)
    }
    readonly property string sourceName: folderActive ? folderName() : catalogSourceName()
    readonly property string phaseLabel: folderActive
        ? QbzSession.tr("Scanning library", QbzSession.trRev)
        : (catalog.phase === "bootstrap"
            ? QbzSession.tr("Building library index", QbzSession.trRev)
            : QbzSession.tr("Updating library index", QbzSession.trRev))
    readonly property string sourceOrdinal: sourceCount > 1 && sourceIndex > 0
        ? " · " + sourceIndex + "/" + sourceCount : ""

    visible: active
    height: 42

    Text {
        id: statusText
        width: parent.width
        height: 15
        text: root.phaseLabel + " · " + root.sourceName + root.sourceOrdinal
        color: theme.textSecondary
        font.pixelSize: 11
        font.weight: theme.weightMedium
        elide: Text.ElideMiddle
        verticalAlignment: Text.AlignVCenter
    }

    Item {
        id: sourceLine
        y: 17
        width: parent.width
        height: 10
        Text {
            id: sourceLabel
            width: 42
            anchors.verticalCenter: parent.verticalCenter
            text: QbzSession.tr("Source", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: 9
        }
        Rectangle {
            anchors.left: sourceLabel.right
            anchors.right: sourceCountText.left
            anchors.leftMargin: 6
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            height: 3
            radius: 2
            color: theme.surfaceElevated
            clip: true
            Rectangle {
                width: parent.width * root.boundedFraction(root.sourceDone, root.sourceTotal)
                height: parent.height
                radius: parent.radius
                color: theme.accent
            }
        }
        Text {
            id: sourceCountText
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 78
            horizontalAlignment: Text.AlignRight
            text: Math.round(root.sourceDone) + " / " + Math.round(root.sourceTotal)
            color: theme.textMuted
            font.pixelSize: 9
        }
    }

    Item {
        y: 29
        width: parent.width
        height: 10
        Text {
            id: overallLabel
            width: 42
            anchors.verticalCenter: parent.verticalCenter
            text: QbzSession.tr("Overall", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: 9
        }
        Rectangle {
            anchors.left: overallLabel.right
            anchors.right: overallCountText.left
            anchors.leftMargin: 6
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            height: 3
            radius: 2
            color: theme.surfaceElevated
            clip: true
            Rectangle {
                width: parent.width * root.boundedFraction(root.overallDone, root.overallTotal)
                height: parent.height
                radius: parent.radius
                color: theme.accent
                opacity: 0.72
            }
        }
        Text {
            id: overallCountText
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 78
            horizontalAlignment: Text.AlignRight
            text: Math.round(root.overallDone) + " / " + Math.round(root.overallTotal)
            color: theme.textMuted
            font.pixelSize: 9
        }
    }
}
