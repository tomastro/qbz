// MiniWindow — the miniplayer's top-level window (2026-08-03 miniplayer/tray
// contract A-27, §4.1), port of `crates/qbz-ui/ui/app.slint:282-305`.
//
// A SECOND top-level Window in the SAME QQmlApplicationEngine, created lazily
// by Main.qml. That route is what deletes the reference's whole mirror layer:
// Slint globals are per-component-tree, so `crates/qbz/src/miniplayer.rs`
// copies 39 NowPlayingState scalars on every 450 ms tick; a Qt #[qml_singleton]
// is one object per engine, so this window reads the SAME QbzPlayer / QbzQueue /
// QbzLyrics / QbzShell the shell reads. MEASURED, not assumed (contract §16
// U-1): the probe build showed one Text bound to QbzPlayer.npTitle following
// the main window across a track change with no mirror code at all.
//
// The window is TRANSPARENT and the visible card is inset by a 6 px gutter, so
// every geometry number in §4.2-§4.5 is relative to the CARD, not to the
// window (§15 trap 3).
//
// NOT ported: the reference's CREATING_MINI double-latch
// (miniplayer.rs:128-130, :166-169). It exists because a winit surface may be
// created lazily on set_size/show() and a global attributes hook has to see the
// flag; in Qt `flags` is declarative and applies at creation (§4.1).

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

Window {
    id: root

    // §15 trap 1, and it is the one that would bite hardest: a Window declared
    // inside another Window is auto-parented (QQuickWindowQmlImpl carries
    // `updateTransientParent`), and a transient child is tied to its parent's
    // mapping by compositor policy — while hiding the main window is the LAST
    // step of enter() (§4.6). Explicitly null, never inherited.
    transientParent: null

    // Borderless, 1:1 with `no-frame: true` (app.slint:293), plus always-on-top
    // — owner ruling K1, §13-D15, and it DEFAULTS ON.
    //
    // The AOT term is NEW WORK rather than a port: §4.8's two probes found
    // NO always-on-top term. It shipped under owner ruling K1 and was
    // REMOVED on 2026-08-04 after the owner smoked it on real Plasma Wayland:
    // the hint did nothing. A Wayland client cannot raise itself above other
    // clients — there is no such request in the core protocol; it takes a
    // compositor policy or a KWin rule, and Qt has nothing to send. The owner
    // chose removal over a button that lies.
    // WINDOWS ONLY, owner decision D8 (2026-08-27), reversing nothing about
    // K1: that ruling removed always-on-top because the owner smoked it on
    // real Plasma Wayland and the hint did nothing -- a Wayland client cannot
    // raise itself above other clients, there is no such request in the core
    // protocol, and Qt had nothing to send. On Windows the same hint IS
    // HWND_TOPMOST and does exactly what it says.
    //
    // So this is a DELIBERATE PER-PLATFORM DIVERGENCE, not a revert: the pin
    // exists where the window manager can honour it and stays absent where it
    // cannot. Linux and macOS keep the exact flags they had.
    property bool pinnedOnTop: false

    flags: Qt.Window | Qt.FramelessWindowHint
        | ((QbzShell.isWindows && root.pinnedOnTop) ? Qt.WindowStaysOnTopHint : 0)

    // Literal, never translated (app.slint:283 uses a plain string, not @tr).
    title: "QBZ Mini Player"

    // Per-window alpha buffer: QQuickWindow::setColor requests it natively
    // (§3.1). The Slint side needs a vendored femtovg fork for the same effect.
    color: "transparent"

    // --- Geometry (§4.2). Every per-mode value is CACHED into a named
    // readonly property, never a ternary repeated per binding (§7-M2): this is
    // a permanently-visible surface and cost per frame scales with the number
    // of live bindings, not with area.
    //
    // Switching the surface RESIZES THE WINDOW and moves its min-height floor
    // with it (§15 trap 5) — these are plain bindings on QbzMini.surface, so
    // that happens by construction. A WM-driven resize writes width/height from
    // C++, which does not disturb a binding (the same property Main.qml:32-35
    // relies on).
    //
    // ⚠ OPEN, and it is an OWNER-SMOKE item, not a known-good: `height` and
    // `minimumHeight` are sibling bindings re-evaluated off the same
    // `surfaceId` change, and QML gives no guaranteed delivery ORDER between
    // them. On a SHRINK — artwork (540, floor 420) to compact (178, floor 170)
    // — a `height` that lands while the OLD floor is still in force is a
    // request qtwayland clamps to the current minimum, and Qt's later min/max
    // update only ever expands the current size. The card would stay stuck at
    // 420 px showing a 178 px layout.
    //
    // **It is left declarative on purpose.** The obvious fix — lower the floor
    // imperatively, then assign the size — was written and MEASURED to break
    // the resize outright on this platform: breaking the two bindings left the
    // window at its old height with the new surface's layout inside it. The
    // declarative form is what RFB shows working (540 -> 178 -> 540, measured).
    // RFB cannot settle the Wayland question either way: the VNC platform
    // enforces no size constraints at all, which is exactly why the shrink
    // measures clean there. If the owner's smoke shows the compact card
    // refusing to shrink, THAT is this note, and the fix needs a real
    // compositor to develop against.
    readonly property int surfaceId: QbzMini.surface
    readonly property bool expanded: root.surfaceId >= 2
    // The expanded size is the PERSISTED one, exposed by the bridge because
    // Rust cannot reach a QQuickWindow (A-6b). The reference reads the same two
    // prefs on every geometry apply (miniplayer.rs:404).
    // THE SIZE IS FIXED PER SURFACE, and min == max so the window manager
    // cannot change it. Owner ruling, 2026-08-04, from a real failure: a
    // Plasma tiling shortcut (Win+arrow) resized the mini to HALF THE SCREEN,
    // the debounced persist wrote 1720 px into the shared ui_prefs.json, and
    // every expanded surface opened giant from then on with no way back —
    // the mini has no resize grips, so nothing in the app could undo it.
    //
    // Their ask was exact: force the sizes even when the OS says otherwise,
    // but stay sensitive to POSITION. min == max does precisely that: a
    // compositor may still move the window, and a tiling request that would
    // resize it is refused.
    //
    // This drops the reference's user-resizable expanded mini, and with it
    // the persisted pair (contract A-6b / AC-15). No loss in practice: this
    // port never gave the mini a resize handle, so the only thing that could
    // ever write those prefs was the WM — i.e. the bug. `QbzMini.miniWidth`
    // and `miniHeight` are therefore no longer read here; the runaway values
    // an earlier session persisted are ignored and self-heal.
    readonly property int winWidth: 380
    readonly property int winHeight: root.expanded
                                     ? 540
                                     : (root.surfaceId === 0 ? 57 : 178)

    width: root.winWidth
    height: root.winHeight
    minimumWidth: root.winWidth
    maximumWidth: root.winWidth
    minimumHeight: root.winHeight
    maximumHeight: root.winHeight

    // NO GEOMETRY PERSIST. The size is fixed above and the WM cannot change
    // it, so there is nothing left to debounce and save — and the persist is
    // exactly what turned one stray tiling shortcut into a permanently giant
    // mini. `QbzMini.saveGeometry` and `mini_qt::geometry_to_persist` stay in
    // the tree with their unit test (UT-3): they are the correct rule for the
    // day a real resize handle exists, and deleting a tested guard to re-derive
    // it later is how this class of bug comes back.


    // A compositor close (Alt+F4, a panel's close entry) behaves EXACTLY like
    // the capsule's expand button — §13-D13, and it closes a hole the reference
    // has: Slint has no on_close_requested on the mini, so its default
    // HideWindow leaves OPEN true, the main window hidden and the app alive but
    // invisible. Accepting the event is also what would arm Qt's quit check
    // (§3.1), so `accepted = false` is load-bearing twice.
    onClosing: function (close) {
        close.accepted = false
        QbzMini.exit()
    }

    // --- The key host (§4.9, A-15) ----------------------------------------
    // BOTH halves are required. Qt delivers key events only to activeFocusItem
    // and propagates up its parent chain, so a tree with no focus item receives
    // nothing; and `focus: true` ALONE did not survive in a WM-less (VNC)
    // session — measured on AppShell (qml/shell/AppShell.qml:75-81), where the
    // dispatcher got nothing until the first click. The idiom is that file's
    // :72 / :81 / :91-97.
    //
    // NOTHING is handled locally: the ordered table lives in Rust behind
    // QbzMini.keyPressed, which resolves through hotkeys_qt::action_for_mini_key
    // (A-16) over the user's ACTIVE bindings. No text-input gate is computed
    // here because the mini has no text fields, exactly as the reference's
    // dispatch_mini has no such guard (crates/qbz/src/keybindings.rs:623-648).
    Item {
        id: keyHost
        anchors.fill: parent
        focus: true
        Component.onCompleted: keyHost.forceActiveFocus()
        Keys.onPressed: function (event) {
            event.accepted = QbzMini.keyPressed(event.key, event.modifiers, event.text)
        }

        // The card: window MINUS 12 px in both axes, a 6 px transparent gutter
        // (app.slint:299-304). §15 trap 3.
        MiniShell {
            id: card
            x: 6
            y: 6
            width: root.width - 12
            height: root.height - 12
            // §8 rule 3: children reach the window through THIS explicit
            // reference, never through parent.parent — a chain qmllint cannot
            // verify. Same seam as Main.qml -> AppShell.hostWindow. The footer's
            // drag handle and the capsule (B3) are its consumers.
            hostWindow: root
        }
    }
}
