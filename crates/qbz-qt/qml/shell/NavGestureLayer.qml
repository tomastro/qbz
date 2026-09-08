// Window-level history-navigation gestures — the QML port of the #653
// handler in crates/qbz-ui/ui/shell/AppShell.slint:244-321.
//
// Two input paths, ONE nav. Both end in QbzShell.navigateBack /
// navigateForward — the same calls the three sacred header buttons make —
// and both honour QbzShell.canBack / canForward, so a back gesture with no
// history does nothing, silently (no beep, no flash, no half-state).
//
//   1. Mouse side buttons (Qt.BackButton / Qt.ForwardButton). They must
//      work anywhere in the window, which is why this is a window-level
//      layer and not something inside HeaderBar.
//   2. Two-finger horizontal touchpad swipe.
//
// STACKING IS LOAD-BEARING — mount this as the FIRST child of the shell
// root so the whole UI paints and hit-tests on top of it (see GLUE NEEDED
// in the round report). Qt delivers a wheel event front-to-back through
// every item under the cursor until one accepts it
// (QQuickDeliveryAgentPrivate::deliverSinglePointEventUntilAccepted), so a
// scroll only reaches this bottom layer when nothing above consumed it:
//
//   * a HORIZONTAL carousel Flickable/ListView CAN flick on x, so it
//     accepts horizontal deltas -> the rail scrolls and no navigation
//     fires. This is the whole point of the bottom placement;
//   * a vertical-only page scroller cannot flick on x, and Qt's Flickable
//     only accepts an axis it can actually move with a non-zero delta on
//     that axis, so a pure-horizontal delta is left unaccepted and falls
//     through to here. Slint reaches the same outcome by the same rule
//     (Flickable::is_allowed_scroll_direction rejects when delta.y == 0
//     and the viewport does not overflow horizontally).
//
// The wheel handler ALWAYS leaves the event unaccepted, on every path out,
// so no scroll routing anywhere in the shell changes because of this file.

import QtQuick
import com.blitzfc.qbz

MouseArea {
    id: gestures

    // Side buttons ONLY. Every other button is not in acceptedButtons, so
    // Qt never even targets this layer for them and the UI above keeps its
    // clicks untouched.
    acceptedButtons: Qt.BackButton | Qt.ForwardButton
    // No hover grab and no cursorShape assignment: this layer must stay
    // invisible to hover states and to the cursor stack above it.
    hoverEnabled: false
    // Explicit even though it is the default — without it MouseArea drops
    // phased (touchpad) wheel events straight to QQuickItem and onWheel
    // never runs.
    scrollGestureEnabled: true

    // --- Tunables (Slint parity) -----------------------------------------
    // 70 logical px accumulated in one gesture — AppShell.slint:297. High
    // enough that resting fingers or small trackpad noise never trip it,
    // low enough that a deliberate swipe crosses it in one motion.
    readonly property real swipeThreshold: 70
    // Gesture-boundary fallback for platforms that report no scroll phase
    // (xcb/XI2 smooth scrolling sends Qt.NoScrollPhase) — AppShell.slint:275
    // uses the same 300 ms idle timer, because Slint's PointerScrollEvent
    // carries no phase at all.
    readonly property int swipeIdleMs: 300
    // Notched-wheel fallback: logical px per wheel notch. Slint's winit
    // backend converts a line delta with `LineDelta(lx, ly) => (lx * 60.,
    // ly * 60.)` (i-slint-backend-winit event_loop.rs:397), and one Qt
    // notch is 120 angle units == one line, so 60 px keeps a horizontal
    // tilt-wheel behaving exactly like it does in the Slint build (two
    // notches to cross the 70 px threshold).
    readonly property real pxPerNotch: 60
    readonly property int anglePerNotch: 120

    // --- Gesture accumulator ---------------------------------------------
    // One navigation per gesture. Deltas arrive as wheel events; the
    // gesture boundary is the scroll phase when the platform provides one,
    // and otherwise the idle timeout below. A direction flip also starts a
    // fresh gesture, and `swipeFired` suppresses re-triggers until the
    // gesture ends.
    property real swipeAccum: 0
    property bool swipeFired: false

    Timer {
        id: swipeIdle
        interval: gestures.swipeIdleMs
        repeat: false
        onTriggered: gestures.resetSwipe()
    }

    function resetSwipe() {
        gestures.swipeAccum = 0
        gestures.swipeFired = false
        swipeIdle.stop()
    }

    // --- Mouse side buttons ----------------------------------------------
    onPressed: function (mouse) {
        if (mouse.button === Qt.BackButton) {
            if (QbzShell.canBack)
                QbzShell.navigateBack()
        } else if (mouse.button === Qt.ForwardButton) {
            if (QbzShell.canForward)
                QbzShell.navigateForward()
        }
    }

    // --- Two-finger horizontal swipe --------------------------------------
    onWheel: function (wheel) {
        // MouseArea sets the event ACCEPTED the moment an onWheel handler is
        // connected (QQuickMouseArea::wheelEvent -> we.setAccepted(
        // isWheelConnected())), so clearing it is mandatory and has to
        // happen before any early return — otherwise this layer would
        // swallow every scroll the views above it declined.
        wheel.accepted = false

        // Qt.ScrollBegin / ScrollEnd / ScrollMomentum are Qt::ScrollPhase,
        // registered on the QML Qt object via Q_ENUM_NS(ScrollPhase)
        // (qnamespace.h:1855 in the Qt 6.11 headers on this box).
        if (wheel.phase === Qt.ScrollBegin) {
            gestures.resetSwipe()
        } else if (wheel.phase === Qt.ScrollEnd) {
            gestures.resetSwipe()
            return
        } else if (wheel.phase === Qt.ScrollMomentum) {
            // Kinetic tail after the fingers lift is not a new gesture, and
            // it must never fire a second navigation.
            return
        }

        // Touchpads report pixel deltas in logical px (the same unit as
        // Slint's delta-x). A notched wheel reports angle deltas only.
        var dx = wheel.pixelDelta.x
        var dy = wheel.pixelDelta.y
        if (dx === 0 && dy === 0) {
            dx = wheel.angleDelta.x / gestures.anglePerNotch * gestures.pxPerNotch
            dy = wheel.angleDelta.y / gestures.anglePerNotch * gestures.pxPerNotch
        }

        // Only clearly horizontal-dominant deltas count toward the gesture;
        // vertical scroll jitter is ignored (AppShell.slint:285).
        if (dx === 0 || Math.abs(dx) <= Math.abs(dy))
            return

        // A direction flip starts a fresh gesture.
        if (gestures.swipeAccum !== 0 && (gestures.swipeAccum > 0) !== (dx > 0)) {
            gestures.swipeAccum = dx
            gestures.swipeFired = false
        } else {
            gestures.swipeAccum += dx
        }
        swipeIdle.restart()

        if (gestures.swipeFired || Math.abs(gestures.swipeAccum) < gestures.swipeThreshold)
            return

        // DIRECTION (macOS/GNOME natural-scrolling convention, identical to
        // AppShell.slint:298-312): fingers RIGHT = back, fingers LEFT =
        // forward. With libinput natural scrolling ON (the GNOME default)
        // fingers right yield a NEGATIVE horizontal delta, hence the signs.
        // Qt and Slint share this sign convention — both add the delta to
        // the content/viewport offset — so the mapping ports 1:1.
        //
        // Users who run WITHOUT natural scrolling flip the mapping with
        // `invert-swipe-navigation` (Slint AppearanceState). The Appearance
        // row for it has always been drawn; until 2026-08-04 this file
        // hard-coded the pref's default and carried a comment claiming no
        // row existed — the toggle really was a no-op, just not for the
        // reason the comment gave.
        const back = QbzShell.invertSwipeNavigation
            ? gestures.swipeAccum > 0
            : gestures.swipeAccum < 0
        if (back) {
            if (QbzShell.canBack)
                QbzShell.navigateBack()
        } else {
            if (QbzShell.canForward)
                QbzShell.navigateForward()
        }
        gestures.swipeFired = true
        gestures.swipeAccum = 0
    }
}
