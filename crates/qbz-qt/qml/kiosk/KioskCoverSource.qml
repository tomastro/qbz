// Mounted-cell request through the existing Rust artwork/cache resolver.
import QtQuick
import QtQuick.Window
import "../assets/kiosk-art.js" as ArtPolicy
import com.blitzfc.qbz

Item {
    id: root
    width: 0; height: 0; visible: false
    property string remote: ""
    property string local: ""
    property real edge: 128
    property string path: ""
    property string lastRequest: ""
    property bool mounted: false
    readonly property string source: local || path
    readonly property string requestUrl: sizedUrl(remote, edge * Screen.devicePixelRatio)
    function sizedUrl(url, pixels) { return ArtPolicy.sizedUrl(url, pixels) }
    function request() {
        if (!mounted || local || requestUrl === lastRequest) return
        path = ""
        lastRequest = requestUrl
        if (requestUrl) QbzShell.sidebarArtworkWindow(JSON.stringify([requestUrl]))
    }
    onRequestUrlChanged: request()
    onLocalChanged: request()
    Component.onCompleted: { mounted = true; request() }
    Connections {
        target: QbzLibrary
        function onLibraryArtworkReady(key, value) {
            if (key === root.requestUrl) root.path = value
        }
    }
}
