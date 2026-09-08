// Library Folders — the folder manager as a FULL-PAGE view (route
// "libraryfolders").
//
// Owner, 2026-08-21: "Library folders se convierte en una subsección como
// Blacklist > Blacklist manager, así tenemos más espacio."
//
// The folder table is the tallest block in Settings by a wide margin — a
// toolbar, a filter, a row per folder and the scan progress — and it sat at
// the TOP of Local Library, pushing Albums view, Library › All, Maintenance,
// Danger zone and the three media servers below the fold. Moving it behind a
// "Manage" row is exactly the shape Blacklist already uses
// (settings/BlacklistSettings.qml -> views/BlacklistManagerView.qml), so this
// file is that view's twin and deliberately reads like it.
//
// NOTHING about the table itself changed: settings/LibraryFolderTable.qml is
// mounted here verbatim, still driven by the settings document and still
// acting through the `settingsString` action keys (settings_qt/library.rs).
// The page is a host, not a fork.
//
// TWO things had to come WITH it, because they are overlays that cannot live
// inside a scrolled panel and were previously supplied by SettingsView:
//   - LibFolderEditModal — the per-folder pencil. Without it here the pencil
//     would open nothing, silently.
//   - the refresh on entry — SettingsView's Local Library panel re-reads the
//     folder list every time it is shown (`onVisibleChanged`), which is what
//     makes a folder added in another session appear. A view reached by Back
//     runs no per-view load (nav_qt::back republishes `currentView` and
//     nothing else), so the refresh is in Component.onCompleted, the same
//     place BlacklistManagerView puts its reload.
//
// No per-view Back chrome: nav history is the global HeaderBar in this port
// (ADR-004 is satisfied there), same as BlacklistManagerView and SettingsView.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../settings"
import "../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    // Transparent while the ambient background is active — the frosted content
    // panel shows through (HomeView.qml:53 and its siblings).
    color: root.ambientOn ? "transparent" : theme.surfaceMain
    readonly property bool ambientOn: theme.ambientOn

    // Round to the AppShell content-frame bezel (Radius.md): QML clips are
    // rectangular, so the frame's rounding never reaches the view.
    radius: 12

    QbzTheme { id: theme }

    /// The settings document, read the same way SettingsView reads it. This
    /// view is mounted by the router, NOT by SettingsView, so it cannot be
    /// handed the parsed copy.
    property var doc: ({})
    readonly property var lib: root.doc.library || ({})

    function reload() {
        try {
            root.doc = JSON.parse(QbzBridge.settingsJson)
        } catch (e) {
            root.doc = ({})
        }
    }
    Component.onCompleted: {
        root.reload()
        // Re-read the folder list on entry — the Slint's
        // `LibraryManageActions.load()`, and what LocalLibrarySettings does
        // in its own `onVisibleChanged`.
        QbzBridge.settingsString("refresh", "")
    }
    Connections {
        target: QbzBridge
        function onSettingsJsonChanged() { root.reload() }
    }

    // --- Header (92px, the Settings title band's metrics) -----------------
    Item {
        id: header
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: root.kioskHost ? 64 : 92
        Text {
            x: root.kioskHost ? 16 : 32
            y: root.kioskHost ? 14 : 23
            text: QbzSession.tr("Library folders", QbzSession.trRev)
            color: theme.textPrimary
            font.pixelSize: theme.fontTitle
            font.weight: theme.weightBold
        }
    }

    Item {
        anchors.top: header.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom

        Flickable {
            id: flick
            anchors.fill: parent
            contentWidth: width
            contentHeight: col.height + 60
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: col
                x: root.kioskHost ? 16 : 32
                y: 4
                width: flick.width - (root.kioskHost ? 32 : 72)
                spacing: 4

                LibraryFolderTable {
                    kioskHost: root.kioskHost
                    width: parent.width
                    lib: root.lib
                }
            }
        }

        // Back/forward scroll memory, like every other routed page.
        ScrollMemory { target: flick; scope: "libraryfolders" }
        QbzScrollBar {
            target: flick
            anchors.right: parent.right
            anchors.rightMargin: 2
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }

    // The per-folder settings modal. It must overlay the whole view, so it is
    // declared last and outside the Flickable — the same reasoning, and the
    // same mount, SettingsView.qml documents for its own copy.
    LibFolderEditModal { kioskHost: root.kioskHost; doc: root.doc }
}
