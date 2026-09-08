// Bottom NavRail — the kiosk's small-panel touch navigation.
// QML port of crates/qbz-ui/ui/shell/NavRail.slint (193 lines).
//
// Seven icon+label tiles (Radius.sm rectangles, NO pills — ADR-008) replace
// the desktop Sidebar and drive the SAME shell routes. Tiles are positioned
// MANUALLY (absolute x + an explicit width derived from the rail's width),
// 1:1 with the reference: NavRail.slint:4-8 records that every layout-based
// attempt left them collapsed to icon width on the left, so the arithmetic
// below is deliberately kept literal rather than handed to a Row.
//
// Search is intentionally absent (NavRail.slint:86-88): the kiosk back bar
// carries a persistent search field, so a footer Search tile is redundant.
//
// The tile ROUTES are the shell's business — every tile emits a signal and
// KioskShell.qml maps it (NavRail.slint:62-68 -> KioskShell.slint:637-657).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    id: root

    // NavRail.slint:70.
    color: theme.surfaceMain

    // NavRail.slint:75 (`out property <int> tile-count: 7`). The nav model
    // carries the same 7 for its rail clamp (src/kiosk_nav_qt.rs `rail_count`).
    readonly property int tileCount: 7

    // NavRail.slint:62-68.
    signal discover()
    signal library()
    signal localLibrary()
    signal myqbz()
    signal playing()
    signal visualizer()
    signal menu()

    /// NavRail.slint:76-84. Enter in the rail zone calls this DIRECTLY (the
    /// rail is always mounted, so it needs no activate-seq pulse); an index
    /// outside 0..6 is silently ignored, exactly like the reference's
    /// else-less if-chain.
    function activateIndex(i) {
        if (i === 0)
            root.discover()
        else if (i === 1)
            root.library()
        else if (i === 2)
            root.localLibrary()
        else if (i === 3)
            root.myqbz()
        else if (i === 4)
            root.playing()
        else if (i === 5)
            root.visualizer()
        else if (i === 6)
            root.menu()
    }

    QbzTheme { id: theme }

    function trs(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // Tile 3's label is the MyQBZ BRANDING, not a msgid (NavRail.slint:145
    // binds `MyQbzBrandingState.label`). Same guarded parse, same `label`
    // field and same fallback as the desktop nav catalog
    // (shell/NavFlyout.qml:85-103) — the shape is not re-derived here.
    readonly property var branding: root.parseBranding()
    function parseBranding() {
        try {
            return JSON.parse(QbzMyQbz.brandingJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property string brandingLabel: {
        var l = root.branding.label || ""
        return l !== "" ? l : root.trs("My QBZ")
    }

    // NavRail.slint:89-90 — 8px outer padding each side, 6 gaps of 6px, so
    // tw = (width - 16 - 36) / 7 and th = height - 18 (62px at the shell's
    // 80px rail).
    readonly property real tw: (root.width - 16 - 6 * 6) / 7
    readonly property real th: root.height - 16

    // The alpha ramp is empty on the pre-publish frame, where alphaTier()
    // silently returns "transparent" — the tree's required guard idiom
    // (rows/TrackRow.qml:264-269 writes out the reason).
    readonly property color tileHoverBg: theme.alphaTiers.length > 0
        ? theme.alphaTier(6)
        : (theme.isDark ? "#0fffffff" : "#0f000000")

    // Tile order == the KioskNavState index inside the rail zone, and the
    // argument activateIndex() dispatches on. Icons: NavRail.slint:105, 118,
    // 131, 144, 159, 172, 185.
    readonly property var tileIcons: ["compass", "library-big", "hard-drive",
        "music-library-2", "play-fill", "sliders-horizontal", "settings"]

    /// The tile labels. `rev` and `brand` are passed by the call site purely
    /// so the delegate's binding re-runs on a language switch and on a
    /// branding change (the same reason NavFlyout.qml:107 threads `trRev`).
    function tileLabel(i, rev, brand) {
        if (i === 0)
            return root.trs("Discover")
        if (i === 1)
            return root.trs("Library")
        if (i === 2)
            return root.trs("Local Library")
        if (i === 3)
            return brand
        if (i === 4)
            return root.trs("Now Playing")
        if (i === 5)
            return root.trs("Visualizer")
        return root.trs("Settings")
    }

    /// The `active` rules (NavRail.slint:107, 120, 133, 146-148, 161, 174,
    /// 187), translated to the Qt route ids (src/nav_qt.rs:18-29). Three of
    /// them are COMPOUND and one is a constant:
    ///   - Local Library is active on the local-album detail too;
    ///   - MyQBZ is active on mixtapes, collections AND the mixtape detail;
    ///   - Visualizer is `false` ALWAYS (NavRail.slint:174) — the immersive
    ///     overlay is not a route, so the tile never lights.
    /// `view` is passed in so the binding re-runs on every route change.
    function tileIsActive(i, view) {
        if (i === 0)
            return view === "home"
        if (i === 1)
            return view === "library"
        if (i === 2)
            return view === "local" || view === "localalbum"
        if (i === 3)
            return view === "mixtapes" || view === "collections"
                || view === "mixtapedetail"
        if (i === 4)
            return view === "nowplaying"
        if (i === 5)
            return false
        return view === "settings"
    }

    // Top border (NavRail.slint:92-98): 1px, full width, declared FIRST so
    // the tiles paint over it.
    Rectangle {
        x: 0
        y: 0
        width: root.width
        height: 1
        color: theme.borderSubtle
    }

    Repeater {
        model: root.tileIcons

        delegate: Rectangle {
            id: tile

            required property int index
            required property string modelData

            // NavRail.slint:16 / :18 — `active` drives the glyph and label
            // colour, `navFocused` drives ONLY the fill and the border. A
            // focused-but-inactive tile therefore keeps its muted glyph and
            // muted label; that asymmetry is deliberate (NavRail.slint:39,46).
            readonly property bool isActive: root.tileIsActive(tile.index, QbzShell.currentView)
            readonly property bool navFocused: QbzKioskNav.navActive
                && QbzKioskNav.zone === "rail"
                && QbzKioskNav.index === tile.index

            // NavRail.slint:101/114/127/140/155/168/181 (x) + y: 9px.
            x: 8 + tile.index * (root.tw + 6)
            y: 9
            width: root.tw
            height: root.th

            // The visual matrix, NavRail.slint:21-28, as a strict priority
            // chain.
            radius: theme.radiusSm
            color: tile.navFocused
                ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.22)
                : tile.isActive
                    ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.12)
                    : (tileArea.containsMouse ? root.tileHoverBg : "transparent")
            border.width: tile.navFocused ? 2 : tile.isActive ? 1 : 0
            border.color: tile.navFocused
                ? theme.accent
                : Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.30)

            // NavRail.slint:30-52 — centred column, 5px between the 24px
            // glyph and the 11.5px/600 label.
            Column {
                id: tileBody
                anchors.centerIn: tile
                width: tile.width
                spacing: 5

                QbzIcon {
                    anchors.horizontalCenter: tileBody.horizontalCenter
                    name: tile.modelData
                    width: 24
                    height: 24
                    tintName: tile.isActive ? "accent" : "muted"
                }
                Text {
                    width: tileBody.width
                    text: root.tileLabel(tile.index, QbzSession.trRev, root.brandingLabel)
                    color: tile.isActive ? theme.accent : theme.textMuted
                    // NavRail.slint:47 is 11.5px, which Qt cannot express:
                    // font.pixelSize is an INT property, and font.pointSize —
                    // the only real-valued alternative — is DPI-relative, so it
                    // would make this one label scale differently from every
                    // other in the tree. Rounded up.
                    font.pixelSize: 12
                    font.weight: theme.weightSemibold
                    horizontalAlignment: Text.AlignHCenter
                    elide: Text.ElideRight
                }
            }

            // NavRail.slint:54-58 — no enabled gate, no pressed state.
            MouseArea {
                id: tileArea
                anchors.fill: tile
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: root.activateIndex(tile.index)
            }
        }
    }
}
