// Shared touchpad kinetic tail for a Flickable/ListView/GridView.
//
// Qt 6.11 already owns the direct pixel-delta drag while the fingers are on
// the pad, including nested-axis arbitration and bounds. On Linux/Wayland the
// gesture commonly ends with ScrollEnd and NO compositor momentum stream, so
// Qt stops the content on the same frame the fingers lift. This component only
// observes that native gesture and hands its final vertical velocity back to
// Flickable.flick(). It never writes contentY and never owns an animation.
//
// RESOURCE CONTRACT (GPU-COST-INVESTIGATION.md): WheelHandler is event-driven;
// there is no Timer, Behavior or permanent clock. The one callLater crosses
// Qt's ScrollEnd cleanup, then Flickable's bounded native timeline owns motion
// and stops itself. At rest this item writes nothing and schedules no frame.

import QtQuick

Item {
    id: root

    required property Flickable target
    /// User-originated wheel/touchpad takeover. Consumers with an opt-in
    /// auto-follow mode use this to yield immediately; programmatic
    /// `positionViewAtIndex` calls never emit it.
    signal userScrollStarted()

    // QbzScrollBar declares us beside the view. Reparent to the Flickable
    // explicitly so this observer covers its whole VIEWPORT, including child
    // rails and cards, rather than the scrollbar's 14px gutter.
    parent: target
    anchors.fill: parent
    z: 2147483646

    property real _sumX: 0
    property real _sumY: 0
    property real _velocityY: 0
    property double _lastMs: 0
    property bool _vertical: false
    property bool _horizontal: false
    property bool _nativeMomentum: false
    property int _generation: 0

    function _resetGesture() {
        root._sumX = 0;
        root._sumY = 0;
        root._velocityY = 0;
        root._lastMs = Date.now();
        root._vertical = false;
        root._horizontal = false;
        root._nativeMomentum = false;
        ++root._generation;
    }

    function _sample(dx, dy) {
        const now = Date.now();
        // A suspended event loop must not turn one stale delta into an absurd
        // velocity; 1..80ms covers the useful gesture sample window.
        const dt = Math.max(1, Math.min(80, now - root._lastMs));
        root._lastMs = now;
        root._sumX += dx;
        root._sumY += dy;

        // Match Qt's own nested-Flickable decision: one axis must beat the
        // other 2:1 before it owns the gesture. A horizontal carousel therefore
        // keeps horizontal/diagonal swipes and can never launch the page tail.
        if (!root._vertical && !root._horizontal) {
            if (Math.abs(root._sumY) > Math.abs(root._sumX) * 2)
                root._vertical = true;
            else if (Math.abs(root._sumX) > Math.abs(root._sumY) * 2)
                root._horizontal = true;
        }

        if (root._vertical) {
            const instant = dy * 1000 / dt;
            // Recent samples matter most at finger lift, without letting one
            // noisy event replace the entire gesture estimate.
            root._velocityY = root._velocityY === 0 ? instant : root._velocityY * 0.35 + instant * 0.65;
        }
    }

    function _launchTail() {
        let velocity = root._velocityY;
        const serial = root._generation;
        if (!root._vertical || root._nativeMomentum || Math.abs(root._sumY) < 12 || Math.abs(velocity) < 220)
            return;

        // Respect each view's platform-tuned ceiling. `-1` means unlimited;
        // use the platform's usual 2500 px/s rather than allowing a noisy
        // high-resolution sample to fling tens of screens.
        const cap = root.target.maximumFlickVelocity > 0 ? root.target.maximumFlickVelocity : 2500;
        velocity = Math.max(-cap, Math.min(cap, velocity));

        // WheelHandler observes before Flickable finishes ScrollEnd. Launch on
        // the next event-loop turn or that cleanup would cancel the new tail.
        Qt.callLater(function () {
            if (serial !== root._generation || !root.target || !root.target.visible || !root.target.interactive)
                return;
            root.target.flick(0, velocity);
        });
    }

    WheelHandler {
        target: null
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
        blocking: false
        enabled: root.target.interactive

        onWheel: function (event) {
            // Observer only. Native Flickable still applies the direct delta,
            // arbitrates nested axes and owns bounds/overshoot.
            event.accepted = false;

            if (event.phase === Qt.ScrollBegin) {
                root.userScrollStarted();
                root._resetGesture();
                return;
            }
            if (event.phase === Qt.ScrollMomentum) {
                // macOS and some compositors already provide the kinetic tail.
                // Never stack ours on top of theirs.
                root._nativeMomentum = true;
                return;
            }
            if (event.phase === Qt.ScrollEnd) {
                root._launchTail();
                return;
            }
            if (event.phase !== Qt.ScrollUpdate)
            {
                // Physical mouse wheels usually carry NoScrollPhase rather
                // than Begin/Update/End. They still represent an explicit
                // user takeover and must cancel a consumer's auto-follow.
                if (event.angleDelta.x !== 0 || event.angleDelta.y !== 0
                        || event.pixelDelta.x !== 0 || event.pixelDelta.y !== 0)
                    root.userScrollStarted();
                return;
            }

            // A physical wheel normally has angleDelta only and is handled by
            // Qt's process-wide wheel-deceleration path (main.rs). This half is
            // deliberately limited to high-resolution pixel gestures.
            root.userScrollStarted();
            const dx = event.pixelDelta.x;
            const dy = event.pixelDelta.y;
            if (dx !== 0 || dy !== 0)
                root._sample(dx, dy);
        }
    }
}
