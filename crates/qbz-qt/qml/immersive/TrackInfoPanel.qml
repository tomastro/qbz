// TrackInfoPanel — the SPLIT-right Track Info panel (split-panel == 1; §6.3
// of the 2026-08-02 immersive-port contract), the Qt answer to
// crates/qbz-ui/ui/immersive/ImmersiveTrackInfoPanel.slint.
//
// NOT a re-render: the panel mounts the SHARED qml/shell/TrackInfoBody.qml
// through a SHIM host (the QtObject below) that exposes the exact host API
// TrackInfoBody reads (doc/loading/errorText/credits/creditsLeft/
// creditsRight + close() + openArtist + openLabel + openMusician —
// TrackInfoBody.qml:27,:31,:47,:260,:283,:295,:159,:133,:252,:288,:301).
// The shim parses QbzAlbum.trackInfoJson EXACTLY like
// TrackInfoModal.qml:71-90 (same guarded parse, same even/odd credit
// split — info_modals.rs step_by(2)).
//
// DIVERGENCES (contract §6.3/D10):
//  * The body's 32x32 close X is a visual element Slint's immersive panel
//    doesn't have — close() here is a PANEL-SWITCH to Lyrics
//    (setView(1, mode, 0)), not an overlay exit.
//  * openLabel / openMusician are INERT in the desktop modal (its TODO
//    comments) but LIVE here per the contract: openLabel ->
//    QbzHome.openLabel, openMusician -> QbzArtist.resolveMusician,
//    openArtist -> QbzArtist.openArtist (the existing artist nav,
//    TrackInfoModal.qml:109-114).
//  * PARITY: immersive STAYS OPEN on navigation — the shell navigates
//    UNDER the overlay (Slint main.rs:15549-15586 closes only
//    TrackInfoState, never ImmersiveState.open).
//
// CAVEAT (trap 18): trackInfoJson is ONE global doc — the desktop
// TrackInfoModal and this panel share it; never open both.
//
// Content height: min(vh, clamp(340, vh*0.62, 760)), vertically centered
// (:84-87 — Tauri .centered-layout height clamp(340px, 62vh, 760px)).

import QtQuick
import com.blitzfc.qbz
import "../shell"
import "../theme"

Item {
    id: root

    // --- The shim host (TrackInfoBody's `host` contract) -------------------
    // One QtObject: the parsed document fields + the four actions the body
    // calls as `host.close()` / `host.openArtist(id)` / `host.openLabel(id)`
    // / `host.openMusician(name, role)`.
    QtObject {
        id: host

        // Verbatim TrackInfoModal.qml:71-90.
        readonly property var doc: parseDoc()
        function parseDoc() {
            try {
                return JSON.parse(QbzAlbum.trackInfoJson || "{}")
            } catch (e) {
                return ({})
            }
        }
        readonly property bool loading: {
            try {
                return QbzAlbum.trackInfoLoading === true
            } catch (e) {
                return false
            }
        }
        readonly property string errorText: doc.error || ""
        readonly property var credits: doc.credits || []
        // Even index -> left column, odd -> right (info_modals.rs step_by(2)).
        readonly property var creditsLeft: credits.filter(function (c, i) { return i % 2 === 0 })
        readonly property var creditsRight: credits.filter(function (c, i) { return i % 2 === 1 })

        // close() — the body's X is a panel-switch to Lyrics (D10).
        function close() {
            QbzImmersive.setView(1, QbzImmersive.mode, 0)
        }
        // Navigation — immersive STAYS OPEN (§6.3 parity); the shell
        // navigates UNDER the overlay.
        function openArtist(artistId) {
            if (!artistId || artistId === "")
                return
            QbzArtist.openArtist(artistId)
        }
        function openLabel(labelId) {
            if (!labelId || labelId === "")
                return
            QbzHome.openLabel(labelId)
        }
        function openMusician(name, roleRaw) {
            if (!name || name === "")
                return
            QbzArtist.resolveMusician(name, roleRaw || "")
        }
    }

    // --- Centered, height-capped content (:81-87) --------------------------
    Item {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        width: parent.width
        height: Math.min(root.height, Math.max(340, Math.min(root.height * 0.62, 760)))

        // ---- Empty (no track / not yet loaded) -------------------------
        // Slint :121-129: !loading && error == "" && title == "".
        Text {
            visible: !host.loading && host.errorText === ""
                && ((host.doc.title || "") === "")
            anchors.centerIn: parent
            text: QbzSession.tr("No track selected", QbzSession.trRev)
            color: "#b3ffffff"
            style: Text.Raised
            styleColor: "#b0000000"
            font.pixelSize: 18
        }

        // ---- Loading / error / loaded — the shared body ---------------
        Flickable {
            id: flick
            anchors.fill: parent
            visible: host.loading || host.errorText !== ""
                || ((host.doc.title || "") !== "")
            contentWidth: width
            contentHeight: body.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            TrackInfoBody {
                id: body
                host: host
                width: flick.width
                // The panel sits on the ambient field — fixed light colors
                // + native shadow (2026-08-31 visual cleanup).
                overAmbient: true
            }
        }

        // The Slint mounts a 14px ListScrollbar inset 4px from the right
        // (:271-278 via the modal's idiom).
        QbzScrollBar {
            target: flick
            anchors.right: parent.right
            anchors.rightMargin: 4
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }
}
