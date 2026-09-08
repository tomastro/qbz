// MiniLyricsSurface — surface 4 (2026-08-03 miniplayer/tray contract §4.5),
// port of `crates/qbz-ui/ui/miniplayer/MiniLyricsSurface.slint` (46 L, read
// whole).
//
// THE EMPTY TEST IS ON THE LINE COUNT, NOT ON `status`
// (`MiniLyricsSurface.slint:11`). The mini has no loading state, no error
// state and no offline-miss state — only "lines" and "no lines" (§15 trap 13).
// The sidebar panel's `status === 2 && lines.length > 0` is deliberately NOT
// reproduced here.
//
// THE SLINT VIEW TAKES ITS DATA FROM A GLOBAL; THE QT COMPONENT TAKES IT AS
// PROPS. `LyricsLinesView.qml` declares `lines: []`, `synced: false`,
// `showTranslation: false` and `sync: null` as ordinary properties with inert
// defaults (`:35-39`), so "pass only the sizing overrides" would mount a
// permanently empty, permanently monolingual view. The mount below is §4.5's
// complete nine-binding list and nothing else — the four sizing numbers are
// the only DIVERGENCE from the component's defaults; the five data bindings
// are the mount.
//
// Everything §4.5 leaves out is left out on purpose: `dimmingMode` (2),
// `autoFollow` (true), `uppercase` (false), `fontIndex` (0) and `activeColor`
// (theme.accent) stay at the component's defaults. The SIDEBAR passes those
// five from the user's prefs (`LyricsPanel.qml:255-263`); the Slint mini
// renders with the fixed values baked into its own view instance
// (`miniplayer.rs:519-534`), so leaving the Qt defaults in place is what
// reproduces it.
//
// `seekRequested` stays UNCONNECTED. The signal exists and the sidebar honours
// it, but the Slint mini has no click-to-seek and its own view header records
// the parity reason. An inert handler that does nothing would be a deferral
// artifact, so there is none.
//
// `show-attribution: false` (`LyricsLinesView.slint:298`) is DEAD in the
// reference — declared and read nowhere, the attribution having moved to a
// fixed footer in the sidebar. The Qt component has no such property and MUST
// NOT grow one (§12-P9).
//
// NO COLOUR LITERALS: every colour on this surface is a theme token, so the
// #RRGGBBAA -> #AARRGGBB conversion the port owes Slint literals has nothing
// to convert here.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz
import "../shell"
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    // {status, synced, provider, error, translationAvailable, lines[]}. The
    // property's Rust default is a full-shape document, which is why the
    // sidebar parses it unguarded (`LyricsPanel.qml:36`).
    readonly property var doc: JSON.parse(QbzLyrics.docJson)
    readonly property var lines: root.doc.lines !== undefined && root.doc.lines !== null
                                 ? root.doc.lines : []
    readonly property bool synced: root.doc.synced === true
    // THE gate — count only (see the header).
    readonly property bool hasLines: root.lines.length > 0

    // --- The mini's OWN sync engine (§4.5, §3.2) ---------------------------
    // A SECOND instance, not a share of the sidebar's. It is stateless per
    // instance (`_index`/`_progress`/`_animMs` are file-local) and calls only
    // read-only invokables (`QbzLyrics.activeIndexAt`, `fillFractionAt`), so
    // two instances cannot fight.
    //
    // The gate is the MINI's own open+surface — never the sidebar's
    // `QbzShell.lyricsOpen`, which nothing here writes — and it carries the
    // panel's `synced` term (`LyricsPanel.qml:55`) so the 33/250 ms pump never
    // ticks on an unsynced document. Gating on the mini's own state is exactly
    // what makes the reference's `ensure_lyrics_engine` hack unnecessary: the
    // karaoke advances with the sidebar panel CLOSED.
    // §4.5 spells this gate as open + surface + synced. The window term is
    // ADDED, not substituted, because §7-M8 is a general rule the mini cannot
    // opt out of: "the windowShowing gate must read THE MINI'S OWN WINDOW",
    // called there the single most likely copy-paste bug in this port.
    // `QbzMini.open` is a LOGICAL flag — it stays true when the compositor or
    // the taskbar minimises the mini, and the 33 ms pump would then pull three
    // invokables thirty times a second, animating a window nobody can see.
    // The blessed shape is `AmbientField.qml:42`; reading it off the MINI is
    // the whole point, so `root.Window.window` here is the mini, never the
    // shell.
    readonly property bool windowShowing: root.Window.window !== null
                                          && root.Window.window.visibility !== Window.Hidden
                                          && root.Window.window.visibility !== Window.Minimized

    LyricsSyncEngine {
        id: syncEngine
        gateOpen: QbzMini.open && QbzMini.surface === 4
                  && root.windowShowing && root.hasLines && root.synced
    }

    // Every doc commit resets the ladder and re-lands on the right line — the
    // Slint `kick()` on commit, and the same two calls the sidebar makes
    // (`LyricsPanel.qml:58-66`).
    Connections {
        target: QbzLyrics
        function onDocJsonChanged() {
            syncEngine.reset()
            syncEngine.kick()
        }
    }

    // --- Empty state (MiniLyricsSurface.slint:13-35) -----------------------
    // VerticalLayout { alignment: center; spacing: 10; padding: 24 }: a
    // centred 30x30 mic-vocal glyph in text-muted, then the 13 px label. The
    // 24 px padding is uniform on all four sides, so the column's width is the
    // surface minus 48.
    Column {
        anchors.centerIn: parent
        width: Math.max(0, root.width - 48)
        visible: !root.hasLines
        spacing: 10

        // The icon's own centring wrapper (`:20-22` is a
        // HorizontalLayout { alignment: center }).
        Item {
            width: parent.width
            height: 30
            QbzIcon {
                anchors.horizontalCenter: parent.horizontalCenter
                name: "mic-vocal"
                tintName: "muted"
                width: 30
                height: 30
            }
        }

        Text {
            width: parent.width
            text: QbzSession.tr("No lyrics", QbzSession.trRev)
            color: theme.textMuted
            font.pixelSize: 13
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
        }
    }

    // --- Populated (:37-45) ------------------------------------------------
    // The nine bindings of §4.5, in its order: the five that are the mount,
    // then the four sizing overrides. The 280 px scale the sidebar computes is
    // the SIDEBAR's; this card is 340-380 px wide and owns its own numbers,
    // which is what `sidePadding` and `lineGap` exist for.
    LyricsLinesView {
        anchors.fill: parent
        visible: root.hasLines

        lines: root.lines
        synced: root.synced
        // No toggle of its own: the mini FOLLOWS the sidebar's bilingual state
        // through the shared model, exactly as `mirror_lyrics_gated` copies it
        // (`crates/qbz/src/miniplayer.rs:610-612`).
        showTranslation: QbzLyrics.showTranslation
        sync: syncEngine
        // §13-D5b, a divergence in the port's favour: the reference's mini
        // cannot honour lite-fill (its window's LyricsState instance is never
        // mirrored, so it always pays the animated karaoke sweep). Here the
        // singleton is shared. Do not "fix" it back.
        liteFill: QbzLyrics.liteFill

        sidePadding: 18   // component default 6
        lineGap: 10       // component default 12
        sizeInactive: 13  // == the component default, passed because the
        sizeActive: 15    // reference states both explicitly
    }
}
