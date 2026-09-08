// Settings > Local Library — the QML port of crates/qbz-ui/ui/settings/
// LocalLibrarySettings.slint (the folder half; the PLEX half is the sibling
// PlexSettings.qml, mounted at the bottom exactly where the Slint has it).
//
// Group order is the Slint's: LIBRARY FOLDERS · ALBUMS VIEW · LIBRARY › ALL ·
// MAINTENANCE · DANGER ZONE · the media servers. LIBRARY FOLDERS is a
// one-row SUBSECTION since 2026-08-21 — the table itself lives in
// views/LibraryFoldersView.qml; see the block comment on that row.
//
// Wiring: the folder table + the scan ride the settings document
// (settings_qt/library.rs over `qbz_library`'s own scan engine); the album
// grouping row drives the SAME QbzLocal.setAlbumMode the Local Library
// header dropdown uses, so both stay one setting.
//
// The whole section is shipped, including the three rows this header used to
// list as cut: "Add folder" opens the NATIVE chooser (`library-pick-folder` ->
// rfd), the per-folder pencil opens LibFolderEditModal.qml (mounted at
// SettingsView.qml:330), and LIBRARY > ALL picks the local scope that
// `library_qt::all_local_feed_blocking` now honours.
//
// ONE delta vs the Slint, deliberate: the folder table scrolls with the page
// instead of in a capped 360px inner scroller (a nested Flickable inside the
// settings Flickable is a scroll trap).

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Column {
    property bool kioskHost: false

    id: root

    property var doc: ({})
    readonly property var lib: doc.library || ({})
    /// The view-level SettingsConfirmHost (see SettingsView.qml). Null in
    /// previews — every call site guards, so a preview degrades to the old
    /// unconfirmed behaviour rather than swallowing the click.
    property var confirmHost: null
    /// View-level reorder modal. It must live outside this scrolled Loader so
    /// its scrim covers Settings instead of moving with the panel.
    property var tabOrderModal: null

    QbzTheme { id: theme }

    spacing: 4

    // Re-read the folder list every time the section is shown — the Slint's
    // `LibraryManageActions.load()` in the panel's init (the panel is mounted
    // by an `if section == 4` there, by visibility here).
    onVisibleChanged: if (visible) QbzBridge.settingsString("refresh", "")

    // ========================= LIBRARY FOLDERS ===========================
    // A SUBSECTION as of 2026-08-21 (owner): the toolbar + filter + folder
    // table + scan progress moved to views/LibraryFoldersView.qml behind this
    // row, exactly the shape Blacklist uses for its manager. The table is the
    // tallest block in Settings and it sat at the TOP of this panel, pushing
    // everything below it — Albums view, Library › All, Maintenance, Danger
    // zone and the three media servers — off the first screen.
    //
    // Layout is BlacklistSettings.qml's, verbatim, so the two subsection rows
    // in Settings are the same object: a 64px row, spacing 24, an 18px glyph
    // + a text column at spacing 3 (title text-primary 15 medium over a
    // text-muted LITERAL 12px status), and a right-aligned Manage button.
    Item {
        width: parent.width
        height: root.kioskHost ? 112 : 64

        Row {
            anchors.fill: parent
            spacing: 24

            Row {
                width: Math.max(0, parent.width - 24 - foldersManageBtn.width)
                anchors.verticalCenter: parent.verticalCenter
                spacing: 12

                QbzIcon {
                    anchors.verticalCenter: parent.verticalCenter
                    name: "folder"
                    width: 18
                    height: 18
                    tintName: "secondary"
                }
                Column {
                    // 30 = the 18px glyph + the 12px row spacing.
                    width: Math.max(0, parent.width - 30)
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 3
                    Text {
                        width: parent.width
                        text: QbzSession.tr("Library folders", QbzSession.trRev)
                        color: theme.textPrimary
                        font.pixelSize: root.kioskHost ? (theme.fontBody) * 1.2 : (theme.fontBody)
                        font.weight: theme.weightMedium
                    }
                    Text {
                        width: parent.width
                        // A live scan is the one thing worth saying here that
                        // the count cannot: the manager shows the progress bar
                        // and the file, this row only says it is happening.
                        text: root.lib.scanning === true
                            ? QbzSession.tr("Scanning", QbzSession.trRev)
                            : QbzSession.tr("{} folders", QbzSession.trRev)
                                .replace("{}", (root.lib.folders || []).length)
                        color: theme.textMuted
                        font.pixelSize: root.kioskHost ? (12) * 1.2 : (12)
                        wrapMode: Text.WordWrap
                    }
                }
            }

            SettingsButton { kioskHost: root.kioskHost;
                id: foldersManageBtn
                anchors.verticalCenter: parent.verticalCenter
                text: QbzSession.tr("Manage", QbzSession.trRev)
                trailingIconName: "chevron-right"
                onClicked: QbzBridge.settingsString("library-open-folders", "")
            }
        }
    }

    Item { width: 1; height: 22 }

    // ============================ TAB ORDER ==============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("LIBRARY TABS", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Tab order and default", QbzSession.trRev)
        description: QbzSession.tr("Choose the order of Local Library tabs. The first one opens by default, including when QBZ starts without a Qobuz session.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            text: QbzSession.tr("Customize", QbzSession.trRev)
            trailingIconName: "chevron-right"
            onClicked: if (root.tabOrderModal) root.tabOrderModal.open()
        }
    }

    Item { width: 1; height: 22 }

    // =========================== GENRES VIEW ============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("GENRES VIEW", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Genre filters position", QbzSession.trRev)
        description: QbzSession.tr("Place the chained genre filters above, beside or below the album results.", QbzSession.trRev)
        QbzSelect { kioskHost: root.kioskHost;
            menuWidth: 170
            options: [
                QbzSession.tr("Top", QbzSession.trRev),
                QbzSession.tr("Right", QbzSession.trRev),
                QbzSession.tr("Left", QbzSession.trRev),
                QbzSession.tr("Bottom", QbzSession.trRev)
            ]
            currentIndex: {
                var value = root.doc.genreFiltersPosition || "top"
                var index = ["top", "right", "left", "bottom"].indexOf(value)
                return index >= 0 ? index : 0
            }
            onSelected: function (index) {
                QbzBridge.settingsSelect("genre-filters-position", index)
            }
        }
    }

    Item { width: 1; height: 22 }

    // ============================ ALBUMS VIEW ============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("ALBUMS VIEW", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Album grouping", QbzSession.trRev)
        description: QbzSession.tr("Folders: one card per album folder — best for compilations and box sets. Metadata: split by album + artist tags.", QbzSession.trRev)
        QbzSelect { kioskHost: root.kioskHost;
            menuWidth: 210
            options: [
                QbzSession.tr("Albums by folder", QbzSession.trRev),
                QbzSession.tr("Albums by metadata", QbzSession.trRev)
            ]
            // Same setting as the Albums-tab header dropdown (one store).
            currentIndex: QbzLocal.localAlbumMode === "metadata" ? 1 : 0
            onSelected: function (i) { QbzLocal.setAlbumMode(i === 1 ? "metadata" : "folder") }
        }
    }

    Item { width: 1; height: 22 }

    // =========================== LIBRARY › ALL ===========================
    // The All view's toolbar hard-drive toggle filters local items in or out;
    // THIS row picks what "local" MEANS in that feed (owner 2026-07-24).
    // LocalLibrarySettings.slint:451-473.
    //
    // It rides `QbzLibrary.libraryPrefsJson` rather than the settings document
    // — the key lives in favorites_ui.json with the rest of the Library
    // toolbar state (co-owned with the Slint build), and that document already
    // carries it. Changing the scope re-navigates in the reference
    // (`main.rs:22570`); here the equivalent is reloadLibrary(), because the
    // feed is built once and cached.
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("LIBRARY › ALL", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Local items in Library All", QbzSession.trRev)
        description: QbzSession.tr("Favorited items only: what you hearted. Entire local library: every folder and Plex album, artist and track.", QbzSession.trRev)
        // NO `sm:` — this row used to be the ONE select in Settings drawn at
        // the small size, which read as a rendering bug next to its
        // neighbours (owner 2026-08-21). The control-alignment standard
        // (qbz-nix-docs/guides/native-ui/) has one size for a settings row.
        QbzSelect { kioskHost: root.kioskHost;
            menuWidth: 210
            options: [
                QbzSession.tr("Favorited items only", QbzSession.trRev),
                QbzSession.tr("Entire local library", QbzSession.trRev)
            ]
            currentIndex: {
                var raw = QbzLibrary.libraryPrefsJson
                if (raw && raw.length > 2) {
                    try {
                        return JSON.parse(raw).allLocalScope === "all" ? 1 : 0
                    } catch (e) { /* fall through */ }
                }
                return 0
            }
            onSelected: function (i) {
                QbzLibrary.setLibraryPref("allLocalScope", i === 1 ? "all" : "favorites")
                QbzLibrary.reloadLibrary()
            }
        }
    }

    Item { width: 1; height: 22 }

    // ============================ MAINTENANCE ============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("MAINTENANCE", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Cleanup missing files", QbzSession.trRev)
        description: QbzSession.tr("Remove tracks whose files no longer exist on disk.", QbzSession.trRev)
        Column {
            spacing: 4
            SettingsButton { kioskHost: root.kioskHost;
                text: root.lib.cleaning === true
                    ? QbzSession.tr("Cleaning up...", QbzSession.trRev)
                    : QbzSession.tr("Cleanup", QbzSession.trRev)
                enabled: root.lib.cleaning !== true
                onClicked: QbzBridge.settingsString("library-cleanup-missing", "")
            }
            Text {
                visible: (root.lib.cleanupStatus || "") !== ""
                width: parent.width
                horizontalAlignment: Text.AlignRight
                text: root.lib.cleanupStatus || ""
                color: theme.textMuted
                font.pixelSize: root.kioskHost ? (theme.fontLegal) * 1.2 : (theme.fontLegal)
            }
        }
    }

    Item { width: 1; height: 22 }

    // ============================ DANGER ZONE ============================
    GroupHeader { kioskHost: root.kioskHost; text: QbzSession.tr("DANGER ZONE", QbzSession.trRev) }
    SettingRow { kioskHost: root.kioskHost;
        label: QbzSession.tr("Clear library database", QbzSession.trRev)
        description: QbzSession.tr("Remove all indexed tracks. Your audio files are not deleted.", QbzSession.trRev)
        SettingsButton { kioskHost: root.kioskHost;
            danger: true
            text: root.lib.clearing === true
                ? QbzSession.tr("Clearing...", QbzSession.trRev)
                : QbzSession.tr("Clear all", QbzSession.trRev)
            enabled: root.lib.clearing !== true
            // TWO prompts before anything is dropped — 1:1 with the
            // reference's two rfd dialogs (local_library_settings.rs:607-626).
            // This port used to clear the database on the first click.
            onClicked: {
                if (!root.confirmHost) {
                    QbzBridge.settingsString("library-clear", "")
                    return
                }
                root.confirmHost.askTwice(
                    QbzSession.tr("Clear library database?", QbzSession.trRev),
                    QbzSession.tr("This removes ALL indexed tracks from the database. Your audio files are NOT deleted. You will need to re-scan your folders afterward.", QbzSession.trRev),
                    QbzSession.tr("Clear all", QbzSession.trRev),
                    QbzSession.tr("Are you absolutely sure?", QbzSession.trRev),
                    QbzSession.tr("This action cannot be undone.", QbzSession.trRev),
                    QbzSession.tr("Clear all", QbzSession.trRev),
                    function () { QbzBridge.settingsString("library-clear", "") })
            }
        }
    }

    Item { width: 1; height: 28 }

    // ================================ PLEX ===============================
    // The media servers, ABOVE Plex: Plex is the legacy integration and the
    // one whose cache has not folded into the shared mirror yet, so the two
    // newer ones lead. One component twice — every difference between the
    // protocols is a property here rather than a second file.
    MediaServerSettings { kioskHost: root.kioskHost;
        width: parent.width
        confirmHost: root.confirmHost
        server: "jellyfin"
        title: "Jellyfin"
        state: (root.doc.library || ({})).jellyfin || ({})
        subtitle: QbzSession.tr("Stream your own Jellyfin library, bit-perfect.", QbzSession.trRev)
        urlPlaceholder: "http://192.168.0.10:8096"
        // Jellyfin really can be tested without credentials.
        testHint: QbzSession.tr("Checked before you sign in.", QbzSession.trRev)
        credentialNote: QbzSession.tr("Jellyfin issues a token; your password is not stored.", QbzSession.trRev)
        syncCost: QbzSession.tr("A first sync takes about a minute per 5,000 tracks.", QbzSession.trRev)
    }

    MediaServerSettings { kioskHost: root.kioskHost;
        width: parent.width
        confirmHost: root.confirmHost
        server: "subsonic"
        title: "Subsonic"
        state: (root.doc.library || ({})).subsonic || ({})
        subtitle: QbzSession.tr("Navidrome, Gonic, Airsonic and other Subsonic servers.", QbzSession.trRev)
        urlPlaceholder: "http://192.168.0.10:4533"
        // Subsonic has NO credential-free endpoint, so the test proves less.
        testHint: QbzSession.tr("Only checks that a Subsonic server answers.", QbzSession.trRev)
        credentialNote: QbzSession.tr("Subsonic has no session, so your password is stored to sign each request.", QbzSession.trRev)
        syncCost: QbzSession.tr("Syncing is fast — a few seconds for a large library.", QbzSession.trRev)
    }

    PlexSettings { kioskHost: root.kioskHost;
        width: parent.width
        doc: root.doc
        confirmHost: root.confirmHost
    }
}
