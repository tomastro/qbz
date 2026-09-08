// Section-nav dropdown flyout — ONE component, mounted by BOTH nav hosts
// (HeaderBar.qml and Sidebar.qml). The QML port of Slint's
// crates/qbz-ui/ui/shell/HeaderMenuOverlay.slint + NavMenuItem.slint, plus
// the entry catalog the Slint tree duplicates three times (Sidebar.slint:724,
// HeaderBar.slint:858 full tabs, HeaderBar.slint:974 compact buttons).
//
// WHY ONE FILE OWNS THE CATALOG TOO: Slint keeps three copies of the same
// [NavMenuEntry] literals because its `if` mount sites cannot share an array
// expression. QML can, so `sections` lives here and both hosts read it —
// there is exactly one place to add a section or an entry.
//
// WHY A Popup: Slint mounts HeaderMenuOverlay as the LAST child of the
// AppShell root because a menu hosted inside the 42px header would be clipped
// by the content area (and a PopupWindow would grab the pointer, killing
// hover-switching between tabs). QtQuick.Controls' Popup reparents its
// content to the window overlay, which gives the same "above everything, not
// clipped by my host" placement WITHOUT touching AppShell.qml — and a
// non-modal Popup does not grab hover, so hover-switching works. The
// closed-sidebar playlists flyout in HeaderBar.qml already uses this pattern.
//
// Open/close behaviour is 1:1 with the Slint overlay:
//   - hovering OR clicking a trigger opens that section's menu;
//   - hovering a different trigger overwrites the open menu (instant switch,
//     single-open is automatic — there is one panel);
//   - the menu closes after 1s idle, and the countdown is PAUSED while the
//     pointer rests on the trigger or anywhere on the panel (panel body OR a
//     row — hence the two-flag OR, HeaderMenuOverlay.slint:41);
//   - a press outside, or Escape, closes it.
//
// GAP vs Slint: no drop shadow — the 1px border-muted outline carries the
// separation instead.
//
// SUPERSEDED (2026-07-29), the REASON this gap used to give: "Qt's shadow
// effects (Qt5Compat / QtQuick.Effects MultiEffect) render NOTHING on the
// software path this port runs on". Effects need shaders. This port runs on
// the GPU (OpenGL RHI, measured 2026-07-29); the earlier "renders nothing"
// note came from an offscreen session, which forces the software renderer by
// definition. Where a software path is genuinely possible, detect it with
// `GraphicsInfo.api` rather than assuming it (theme/RoundedImage.qml does).
// The gap itself STAYS for now: adding the shadow is a visual change that
// needs its own parity pass against Slint's `drop-shadow-blur`, not a perf
// fix — do not add one in a domain commit.

import QtQuick
import QtQuick.Controls
import com.blitzfc.qbz
import "../theme"

Item {
    id: nav
    // Zero-size, non-layout host: it exists only to give the Popup a
    // coordinate system (the host root's) and to carry the shared catalog.
    // Mount it as a DIRECT child of the host root, never inside a Row/Column,
    // so its origin stays the host root's origin.
    width: 0
    height: 0

    // ---------------------------------------------------------------------
    // Catalog
    // ---------------------------------------------------------------------

    // Slint gates the Recommendations entry on SettingsState.show-recommendations
    // (discover_prefs `show_recommendations`, default ON) — the port publishes
    // it in QbzBridge.settingsJson.
    readonly property var settingsDoc: {
        try {
            return JSON.parse(QbzBridge.settingsJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property bool showRecommendations:
        nav.settingsDoc.showRecommendations !== false
    readonly property var localTabOrder: {
        var stored = nav.settingsDoc.localTabOrder
        return Array.isArray(stored) && stored.length > 0
            ? stored : ["genres", "albums", "artists", "folders", "tracks"]
    }

    // MyQBZ branding — `label` (user-editable in Settings > Appearance,
    // coerced back to "My QBZ" on the Rust side when cleared), `iconPath` +
    // `hasCustomIcon` for a user-supplied section icon. Published on
    // QbzMyQbz.brandingJson, seeded at construction (not at boot()) precisely
    // because this catalog binds it on the FIRST frame.
    //
    // The parse is GUARDED (the parseDoc idiom, views/PlaylistView.qml:53-60):
    // a raw in-binding JSON.parse throws on the pre-publish frame and takes
    // the whole nav catalog down with it — both hosts included, since both
    // read `sections` from here. The try also covers QbzMyQbz not being
    // registered at all (an unknown type name is a plain JS global lookup, so
    // it raises a catchable ReferenceError rather than a load error).
    readonly property var branding: parseBranding()
    function parseBranding() {
        try {
            return JSON.parse(QbzMyQbz.brandingJson)
        } catch (e) {
            return ({})
        }
    }

    // Rust never publishes an empty label — it coerces a cleared one back to
    // the English literal "My QBZ" — so the localised fallback below is
    // defensive only, and the SHIPPED default is that English literal. That is
    // a pre-existing i18n hole shared with the Slint app
    // (MyQbzBrandingState.label's default is the same literal); do not "fix"
    // it here by second-guessing the Rust value.
    readonly property string brandingLabel: {
        var l = nav.branding.label || ""
        return l !== "" ? l : nav.trs("My QBZ")
    }
    readonly property string brandingIconPath:
        nav.branding.hasCustomIcon === true ? (nav.branding.iconPath || "") : ""

    // The four top-level sections, in Slint order. `rev` is threaded through
    // buildSections purely so the binding re-runs on a language switch
    // (QbzSession.trRev is the translation revision counter every other
    // tr() call site binds to).
    readonly property var sections:
        buildSections(QbzSession.trRev, showRecommendations, localTabOrder)

    function trs(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // Entry shape: { label, icon (baked glyph name, "" = no glyph),
    //                view (shell route), tab (view-internal tab id),
    //                enabled }
    // Section shape: { id, icon, label, qobuz (hidden while offline),
    //                  enabled, entries,
    //                  iconPath — OPTIONAL: absent or "" means "use the baked
    //                  `icon`"; a non-empty path is a user-supplied image
    //                  rendered raw by shell/NavSectionGlyph.qml. Only the
    //                  MyQBZ section can carry one. }
    function buildSections(rev, reco, localOrder) {
        // Discover — glyph-less rows (Slint has-glyph: false).
        var discover = [
            { "label": nav.trs("Home"), "icon": "", "view": "home", "tab": "home", "enabled": true },
            { "label": nav.trs("For You"), "icon": "", "view": "home", "tab": "forYou", "enabled": true },
            { "label": nav.trs("Editor's Picks"), "icon": "", "view": "home", "tab": "editorPicks", "enabled": true }
        ]
        if (reco) {
            discover.push({ "label": nav.trs("Recommendations"), "icon": "", "view": "home", "tab": "recommendations", "enabled": true })
        }
        // Local Library's fixed surfaces have ONE authoritative user order.
        // Build both flyout hosts from it, then append the ephemeral Open
        // submenu: Open is an action/session, not a reorderable landing tab.
        var localById = {
            "albums": { "label": nav.trs("Albums"), "icon": "disc", "view": "local", "tab": "albums", "enabled": true },
            "artists": { "label": nav.trs("Artists"), "icon": "user", "view": "local", "tab": "artists", "enabled": true },
            "genres": { "label": nav.trs("Library Explorer"), "icon": "music-note-slider", "view": "local", "tab": "genres", "enabled": true },
            "folders": { "label": QbzSession.offline ? nav.trs("Folder view") : nav.trs("Folders"),
                         "icon": "folder", "view": "local", "tab": "folders", "enabled": true },
            "tracks": { "label": nav.trs("Tracks"), "icon": "music", "view": "local", "tab": "tracks", "enabled": true }
        }
        var localEntries = []
        var seenLocal = ({})
        for (var li = 0; li < localOrder.length; li++) {
            var localId = localOrder[li]
            if (localById[localId] && seenLocal[localId] !== true) {
                localEntries.push(localById[localId])
                seenLocal[localId] = true
            }
        }
        // Defensive completion for the construction-time/pre-migration frame.
        var localFallback = ["genres", "albums", "artists", "folders", "tracks"]
        for (var lf = 0; lf < localFallback.length; lf++) {
            var fallbackId = localFallback[lf]
            if (seenLocal[fallbackId] !== true)
                localEntries.push(localById[fallbackId])
        }
        localEntries.push({
            "label": QbzLocal.localEphemeralActive
                         ? QbzLocal.localEphemeralLabel
                         : (QbzSession.offline ? nav.trs("Open media") : nav.trs("Open")),
            "icon": "folder-open", "view": "", "tab": "", "enabled": true,
            "hasSubmenu": true, "submenu": nav.openMediaEntries()
        })
        return [
            {
                "id": "discover",
                "icon": "compass",
                "label": nav.trs("Discover"),
                "qobuz": true,
                "enabled": true,
                "entries": discover
            },
            {
                "id": "library",
                "icon": "music-library-2",
                "label": nav.trs("Library"),
                "qobuz": true,
                "enabled": true,
                "entries": [
                    { "label": nav.trs("All"), "icon": "layout-grid", "view": "library", "tab": "all", "enabled": true },
                    { "label": nav.trs("Tracks"), "icon": "music", "view": "library", "tab": "tracks", "enabled": true },
                    { "label": nav.trs("Albums"), "icon": "disc", "view": "library", "tab": "albums", "enabled": true },
                    { "label": nav.trs("Artists"), "icon": "user", "view": "library", "tab": "artists", "enabled": true },
                    { "label": nav.trs("Playlists"), "icon": "list-music", "view": "library", "tab": "playlists", "enabled": true },
                    { "label": nav.trs("Labels"), "icon": "disc-3", "view": "library", "tab": "labels", "enabled": true }
                ]
            },
            {
                "id": "local",
                "icon": "hard-drive",
                "label": nav.trs("Local Library"),
                "qobuz": false,
                "enabled": true,
                "entries": localEntries
            },
            {
                "id": "myqbz",
                "icon": "qbz-symbolic",
                // Slint's HEADER hosts label this tab @tr("My QBZ")
                // (HeaderBar.slint:938, :1048) while only the sidebar and the
                // kiosk rail read MyQbzBrandingState.label (Sidebar.slint:787,
                // NavRail.slint:145). ONE catalog cannot express that split,
                // and a user who renames the section expects the rename
                // everywhere, so both hosts get the branding label.
                "label": nav.brandingLabel,
                // Custom section icon, rendered raw (its own colours, no theme
                // tint) — Slint's SidebarNavRow `raw-icon` arm.
                "iconPath": nav.brandingIconPath,
                "qobuz": false,
                "enabled": true,
                "entries": [
                    // ORDER is the header's: Collections then Mixtapes
                    // (HeaderBar.slint:941-942). Slint's sidebar contradicts
                    // itself with Mixtapes first (Sidebar.slint:790-791), and
                    // one catalog serves both hosts.
                    // ICONS are the header's pair too: cassette-tape +
                    // library-big are baked in all eight tint dirs, while the
                    // sidebar's cassette + collection are not — preferring
                    // them would manufacture two asset imports for a visually
                    // equivalent glyph.
                    { "label": nav.trs("Collections"), "icon": "library-big", "view": "collections", "tab": "", "enabled": true },
                    { "label": nav.trs("Mixtapes"), "icon": "cassette-tape", "view": "mixtapes", "tab": "", "enabled": true }
                ]
            }
        ]
    }

    /// Second-level catalog for Local Library > Open. When a medium already
    /// exists its own row returns to it and Close is appended; all three open
    /// verbs remain available so replacing one medium never needs an extra
    /// navigation round trip.
    function openMediaEntries() {
        var entries = []
        if (QbzLocal.localEphemeralActive) {
            entries.push({ "label": QbzLocal.localEphemeralLabel,
                           "icon": "folder-open", "view": "local",
                           "tab": "ephemeral", "enabled": true })
        }
        entries.push(
            { "label": nav.trs("Open folder…"), "icon": "folder-open",
              "view": "", "tab": "", "action": "openFolder", "enabled": true })
        // W16: qbz-disc has no Windows CDDA backend -- `mod unsupported`
        // compiles and every call returns NoDrive. Offering the entry would
        // only ever produce "no drive found" on a machine that has one. SACD
        // stays: that opens an image file, which needs no drive.
        if (!QbzShell.isWindows) {
            entries.push(
                { "label": nav.trs("Open audio CD"), "icon": "disc",
                  "view": "", "tab": "", "action": "openCd", "enabled": true })
        }
        entries.push(
            { "label": nav.trs("Open SACD image…"), "icon": "disc-3",
              "view": "", "tab": "", "action": "openSacd", "enabled": true })
        if (QbzLocal.localEphemeralActive) {
            entries.push({ "label": nav.trs("Close"), "icon": "x",
                           "view": "", "tab": "", "action": "closeMedia",
                           "enabled": true })
        }
        return entries
    }

    // Which section owns the highlight, derived from the LIVE view (both hosts
    // used to carry this identical binding). Slint highlights off
    // HeaderMenuState.open-index; the hosts OR that in via `openId`.
    //
    // Each alternative spells `QbzShell.currentView === x` out in full: the
    // shorthand `a === x || y || z` is ALWAYS truthy (`"y"` is a non-empty
    // string), which would light the section up on every route in the app.
    // "discobuilder" is deliberately NOT in the MyQBZ arm — Slint's own active
    // predicate omits the builder (NavRail.slint:146-148).
    readonly property string activeSection:
          QbzShell.currentView === "home" ? "discover"
        : QbzShell.currentView === "library" ? "library"
        : (QbzShell.currentView === "local" || QbzShell.currentView === "localalbum") ? "local"
        : (QbzShell.currentView === "mixtapes"
           || QbzShell.currentView === "collections"
           || QbzShell.currentView === "mixtapedetail") ? "myqbz"
        : ""

    // ---------------------------------------------------------------------
    // Open / close (HeaderMenuState)
    // ---------------------------------------------------------------------

    // Section id whose menu is open ("" = none) — the port's open-index.
    // Keyed on `visible` (flips the instant open()/close() runs), not on
    // `opened` (which waits out an enter transition).
    readonly property string openId: panel.visible ? panel.sectionId : ""
    // True while the pointer rests on the TRIGGER. The trigger owns this flag
    // (HeaderMenuOverlay.slint:51): only the trigger that owns the open menu
    // clears it, which avoids a leave/enter race when sliding between triggers.
    property bool triggerHovered: false

    // Header dropdowns hang at a fixed window y (Slint: 45px = the 42px header
    // plus 3), NOT under each tab — the host mounts this at its own origin, so
    // in HeaderBar (top of the window) the two coincide.
    property real dropdownY: 45

    // Header form: menu under the trigger, left edges aligned.
    // `withTitle` = the section-name header row + hairline, which Slint shows
    // for the compact icon buttons (the icon alone does not name the section)
    // and omits for the full text tabs.
    function openUnder(trigger, section, withTitle) {
        if (!section || section.enabled === false) return
        panel.show(section, withTitle === true, trigger.mapToItem(nav, 0, 0).x, nav.dropdownY)
    }

    // Sidebar form: menu to the RIGHT of the row (+6px), aligned to its top.
    // The expanded row already names the section, while the compact icon does
    // not; callers pass that visible-text fact explicitly.
    function openBeside(trigger, section, withTitle) {
        if (!section || section.enabled === false) return
        var p = trigger.mapToItem(nav, trigger.width + 6, 0)
        panel.show(section, withTitle === true, p.x, p.y)
    }

    function closeMenu() { panel.close() }

    // ---------------------------------------------------------------------
    // Entry activation
    // ---------------------------------------------------------------------

    // Entries with a tab go through the bridge seam (navigateToTab), which
    // both navigates and hands the tab to ContentRouter's Binding — the
    // Slint behaviour (on_header_menu_navigate selects the landing tab
    // per route, crates/qbz/src/main.rs:18026-18046). Entries without one
    // (Collections, Mixtapes) are a plain section navigation.
    function activate(entry) {
        if (entry && entry.submenu) return
        panel.close()
        if (!entry || entry.enabled === false) return
        if (entry.action === "openFolder") {
            QbzLocal.ephemeralOpen()
            return
        }
        if (entry.action === "openCd") {
            QbzLocal.ephemeralOpenCd()
            return
        }
        if (entry.action === "openSacd") {
            QbzLocal.ephemeralOpenSacd()
            return
        }
        if (entry.action === "closeMedia") {
            QbzLocal.ephemeralClear()
            return
        }
        if (entry.view === "") return
        if (entry.tab && entry.tab !== "")
            QbzShell.navigateToTab(entry.view, entry.tab)
        else
            QbzShell.navigateTo(entry.view)
    }

    // OPT-IN (Settings > Appearance, OFF by default): a click on a top-level
    // section row also navigates, landing on that section's FIRST entry —
    // Discover on Home, Library on All. Shipped behaviour is that a click only
    // opens the flyout and the user picks from it, which is what `false` keeps.
    //
    // It lives HERE and not in the two hosts because the catalog lives here:
    // "the first tab" is `entries[0]`, and reusing `activate` means the MyQBZ
    // section — whose entries are plain views with no tab — needs no special
    // case. Sidebar.qml and HeaderBar.qml each call this from their row's
    // `onClicked`, next to the open they already do.
    //
    // GUARDED parse, the same precedent as `branding` above: a bare
    // JSON.parse in a binding throws on the pre-publish frame and would take
    // the whole nav catalog — and therefore both hosts — down with it.
    readonly property bool clickNavigates: {
        try {
            return JSON.parse(QbzBridge.settingsJson).navClickFirstTab === true
        } catch (e) {
            return false
        }
    }
    /// Call from a section row's click. A no-op unless the user opted in.
    function sectionClicked(section) {
        if (!nav.clickNavigates || !section || section.enabled === false)
            return
        var entries = section.entries || []
        if (entries.length > 0)
            nav.activate(entries[0])
    }

    QbzTheme { id: theme }

    // Idle-close (project rule: 1s), paused while the pointer rests on the
    // menu or on its trigger. Lives HERE and not inside the Popup: the Popup
    // carries an explicit contentItem, so a non-visual default-property child
    // would be handed to that content item instead of the popup.
    Timer {
        interval: 1000
        repeat: false
        running: panel.visible && !panel.menuHovered
        onTriggered: panel.close()
    }

    // ---------------------------------------------------------------------
    // The panel (HeaderMenuOverlay's Rectangle + NavMenuItem rows)
    // ---------------------------------------------------------------------

    // One menu row — NavMenuItem.slint: 32px, hover surface-hover, 14px glyph
    // (optional), 13px text-secondary label, padding 14 left / 22 right.
    component NavMenuRow: Rectangle {
        id: row
        property var entry: null
        property int rowIndex: -1
        readonly property bool rowEnabled: entry && entry.enabled !== false
        // QVariantList conversion makes runtime array introspection unreliable.
        // The catalog explicitly marks the sole cascading entry instead.
        readonly property bool hasSubmenu: !!(entry && entry.hasSubmenu === true)

        width: parent ? parent.width : 0
        height: 32
        opacity: rowEnabled ? 1.0 : 0.45
        color: (rowEnabled && rowArea.containsMouse) ? theme.surfaceHover : "transparent"

        Row {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 22
            spacing: 9

            QbzIcon {
                visible: row.entry && row.entry.icon !== ""
                width: visible ? 14 : 0
                height: 14
                anchors.verticalCenter: parent.verticalCenter
                name: row.entry ? row.entry.icon : ""
                tintName: "secondary"
            }
            Text {
                height: parent.height
                width: parent.width
                    - (row.entry && row.entry.icon !== "" ? 23 : 0)
                    - (row.hasSubmenu ? 19 : 0)
                text: row.entry ? row.entry.label : ""
                color: theme.textSecondary
                font.pixelSize: 13
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzIcon {
                visible: row.hasSubmenu
                width: visible ? 11 : 0
                height: 11
                anchors.verticalCenter: parent.verticalCenter
                name: "chevron-right"
                tintName: "muted"
            }
        }

        MouseArea {
            id: rowArea
            anchors.fill: parent
            enabled: row.rowEnabled
            hoverEnabled: row.rowEnabled
            cursorShape: Qt.PointingHandCursor
            onClicked: {
                if (row.hasSubmenu)
                    panel.openCascade(row, row.entry)
                else
                    nav.activate(row.entry)
            }
            // Per-row hover keeps the idle timer paused while the cursor RESTS
            // on a row (the panel-body tracker below never sees it — the row
            // is on top of it).
            onContainsMouseChanged: {
                if (containsMouse) {
                    panel.hoveredRow = row.rowIndex
                    if (row.hasSubmenu)
                        panel.openCascade(row, row.entry)
                    else
                        panel.scheduleCascadeClose()
                } else {
                    if (panel.hoveredRow === row.rowIndex) panel.hoveredRow = -1
                    if (panel.cascadeAnchorRow === row.rowIndex)
                        panel.scheduleCascadeClose()
                }
            }
        }
    }

    Popup {
        id: panel
        width: 188
        padding: 0
        topPadding: 6
        bottomPadding: 6
        // Deterministic height (Slint: panel-box.preferred-height) — 32px per
        // row, plus the optional 26px title band and its 1px hairline.
        height: 12 + (panel.title === "" ? 0 : 27) + panel.entries.length * 32
        closePolicy: Popup.CloseOnPressOutside | Popup.CloseOnEscape

        property string sectionId: ""
        property string title: ""
        property var entries: []
        // Which row the pointer is on (-1 = none); rows do not overlap.
        property int hoveredRow: -1
        // True while the pointer rests on the panel BODY (padding/gaps).
        property bool bodyHovered: false
        // The cascade remains live while the pointer crosses the panel edge.
        // 280ms is long enough for a diagonal move but short enough that an
        // abandoned submenu does not become perennial.
        property int cascadeAnchorRow: -1
        property bool cascadeGrace: false
        // Pointer anywhere on the menu, or on the trigger that owns it — the
        // idle countdown is paused while this holds.
        readonly property bool menuHovered:
            bodyHovered || hoveredRow >= 0 || nav.triggerHovered
            || cascade.menuHovered || cascadeGrace

        function openCascade(row, entry) {
            cascadeClose.stop()
            panel.cascadeGrace = false
            panel.cascadeAnchorRow = row.rowIndex
            cascade.entries = entry.submenu || []
            cascade.x = panel.width - 2
            cascade.y = row.mapToItem(panel.contentItem, 0, 0).y
            if (!cascade.visible) cascade.open()
        }

        function scheduleCascadeClose() {
            if (!cascade.visible) return
            panel.cascadeGrace = true
            cascadeClose.restart()
        }

        function show(section, withTitle, px, py) {
            // Switching sections: drop stale hover state (the new rows have
            // not reported yet), exactly like the Slint `changed open-index`.
            if (panel.sectionId !== section.id) panel.hoveredRow = -1
            if (panel.sectionId !== section.id) cascade.close()
            panel.sectionId = section.id
            panel.title = withTitle ? section.label : ""
            panel.entries = section.entries
            panel.x = px
            panel.y = py
            if (!panel.visible) panel.open()
        }

        onClosed: {
            panel.sectionId = ""
            panel.hoveredRow = -1
            panel.bodyHovered = false
            panel.cascadeAnchorRow = -1
            panel.cascadeGrace = false
            cascade.close()
            nav.triggerHovered = false
        }

        background: Rectangle {
            color: theme.surfaceMain
            radius: theme.radiusSm
            border.width: 1
            border.color: theme.borderMuted

            // Panel-body hover tracker. It lives in the BACKGROUND, i.e. below
            // the content item, so it can never swallow a row's press (a fill
            // MouseArea declared over the rows would — a press is not a
            // composed event and propagateComposedEvents does not rescue it).
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                onContainsMouseChanged: panel.bodyHovered = containsMouse
                // Swallow clicks on the panel chrome so they do not fall
                // through to whatever sits under the flyout.
                onClicked: {}
            }
        }

        contentItem: Column {
            spacing: 0

            // Optional section-name header + hairline (Slint: shown for the
            // compact header buttons and every sidebar flyout).
            Text {
                visible: panel.title !== ""
                width: parent.width
                height: visible ? 26 : 0
                leftPadding: 14
                rightPadding: 14
                text: panel.title
                color: theme.textMuted
                font.pixelSize: 11
                font.weight: theme.weightSemibold
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            Rectangle {
                visible: panel.title !== ""
                width: parent.width
                height: visible ? 1 : 0
                color: theme.borderSubtle
            }

            Repeater {
                model: panel.entries
                delegate: NavMenuRow {
                    required property var modelData
                    required property int index
                    entry: modelData
                    rowIndex: index
                }
            }
        }

        Timer {
            id: cascadeClose
            interval: 280
            repeat: false
            onTriggered: {
                panel.cascadeGrace = false
                if (panel.cascadeAnchorRow !== panel.hoveredRow && !cascade.menuHovered)
                    cascade.close()
            }
        }

        // Child Popup, not a sibling: presses inside it count as part of the
        // menu family for CloseOnPressOutside, and closing the parent tears it
        // down automatically. The panels overlap by 2px so there is no dead
        // seam; the timer above tolerates diagonal pointer travel.
        Popup {
            id: cascade
            width: 224
            padding: 0
            topPadding: 6
            bottomPadding: 6
            height: 12 + cascade.entries.length * 32
            closePolicy: Popup.NoAutoClose

            property var entries: []
            property int hoveredRow: -1
            property bool bodyHovered: false
            readonly property bool menuHovered: bodyHovered || hoveredRow >= 0

            onClosed: {
                cascade.entries = []
                cascade.hoveredRow = -1
                cascade.bodyHovered = false
                panel.cascadeAnchorRow = -1
            }

            background: Rectangle {
                color: theme.surfaceMain
                radius: theme.radiusSm
                border.width: 1
                border.color: theme.borderMuted
                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    onContainsMouseChanged: {
                        cascade.bodyHovered = containsMouse
                        if (containsMouse) {
                            cascadeClose.stop()
                            panel.cascadeGrace = false
                        } else {
                            panel.scheduleCascadeClose()
                        }
                    }
                    onClicked: {}
                }
            }

            contentItem: Column {
                Repeater {
                    model: cascade.entries
                    delegate: Rectangle {
                        id: cascadeRow
                        required property var modelData
                        required property int index
                        width: parent ? parent.width : 0
                        height: 32
                        color: cascadeArea.containsMouse ? theme.surfaceHover : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 14
                            anchors.rightMargin: 14
                            spacing: 9
                            QbzIcon {
                                width: 14
                                height: 14
                                anchors.verticalCenter: parent.verticalCenter
                                name: cascadeRow.modelData.icon || ""
                                tintName: "secondary"
                            }
                            Text {
                                width: parent.width - 23
                                height: parent.height
                                text: cascadeRow.modelData.label || ""
                                color: theme.textSecondary
                                font.pixelSize: 13
                                verticalAlignment: Text.AlignVCenter
                                elide: Text.ElideRight
                            }
                        }

                        MouseArea {
                            id: cascadeArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: nav.activate(cascadeRow.modelData)
                            onContainsMouseChanged: {
                                if (containsMouse) {
                                    cascade.hoveredRow = cascadeRow.index
                                    cascadeClose.stop()
                                    panel.cascadeGrace = false
                                } else {
                                    if (cascade.hoveredRow === cascadeRow.index)
                                        cascade.hoveredRow = -1
                                    panel.scheduleCascadeClose()
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
