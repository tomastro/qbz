// VizSettle — ONE shared frame-applier for a whole visualizer stream.
//
// WHAT CHANGED 2026-08-13 (the single-pulse redesign). This object used to
// REPLAY each 30 Hz published frame across the render frames that followed
// it, driven by a private 16 ms Timer (~62 Hz) — the QML answer to the Slint
// dock's per-bar `animate bar-h { 90ms }` (SidebarNowPlayingDock.slint:45).
// That driver was the shell's dominant GPU term: Qt Quick has no dirty-region
// rendering, so every tick repainted the WHOLE window, and 62 ticks/s against
// the atmosphere's own 33 ms Timer — unsynchronised — is what the
// 2026-08-11 GPU investigation measured as ~62 presents/s, render = 2 ms,
// swap = 12-24 ms, 93-97% GPU on the owner's 4070. Interpolating between 30
// Hz publishes is a luxury that only pays when the repaint is a rectangle;
// when the repaint is the window, the spectrum moves at the rate the data
// arrives and that is the deal (GPU-COST-INVESTIGATION.md §4).
//
// THE CONTRACT NOW:
//  - A publish (onTargetChanged) STASHES the frame in a plain JS slot and
//    writes NO bound property — a publish alone never schedules a frame.
//  - The shared shell pulse (QbzShell.pulseMs, shell_bridge.rs) is the ONLY
//    driver. On its edge, and only if a frame is pending, the latest frame is
//    applied: `from`/`to` swap and every `at(i)` binding re-evaluates in the
//    SAME event-loop turn as every other pulse consumer, so the window
//    presents ONCE per period for the whole shell.
//  - SELF-PARKING is structural: a paused player, a hidden band or a
//    disabled tap publishes nothing (viz_qt.rs), nothing goes pending, and
//    the pulse handler writes nothing — zero frames, zero timers to park.
//    The `live` gate below — fed by the dock's `vizShouldRun`, which drops
//    when the window deactivates — keeps an occluded window at zero
//    repaints. Do not weaken it.
//  - Delegates bind ONE call, `at(i)`, which reads `from`, `to` and
//    `progress` unconditionally on every path — QML captures those three as
//    dependencies, so a pulse re-evaluates the bar heights and instantiates
//    nothing.
//
// SMOOTHNESS NOTE, CORRECTED 2026-08-19. This paragraph used to argue that
// dropping the interpolator did not cost smoothness, because qbz-audio already
// band-limits the bars in Rust (attack 0.7 / decay 0.65,
// processor.rs:174-184). That reasoning was wrong about what the eye reads: a
// band-limited signal APPLIED AS A STEP still moves in visible jumps, and the
// owner's side-by-side against the Slint build named it exactly — "no es que
// se vea mas rapido, la transicion entre estados es mas suave". The
// interpolation is back, on the pulse; see `easeK`.
//
// LATENCY: a frame is applied on the first pulse after its publish, i.e. up
// to one period (33 ms) behind the tap. Against the pipeline that already
// exists (46 ms FFT window + 77 ms smoothing) that is not perceptible.

import QtQuick
import com.blitzfc.qbz

Item {
    id: settle

    // ---- input -------------------------------------------------------------
    // Newest published frame, DISPLAY-READY (0..1). Replaced ~30x/s. Whatever
    // per-sample mapping a mode needs (abs, downsample) belongs in the binding
    // that feeds this, so it runs once per publish instead of once per bar per
    // frame.
    property var target: []
    // Master gate — mirror the band's capture gate. false = frozen: the
    // publish path writes NOTHING (an invisible consumer's writes still dirty
    // the whole window — see onTargetChanged), the pulse handler returns
    // early, and the last applied frame holds.
    property bool live: true

    // ---- output (read via at(i)) -------------------------------------------
    //
    // ONE applied array, eased toward the newest published frame. It replaced
    // a from/to/progress triple that only ever held progress = 1.0, i.e. a
    // step — see the EASING note above `easeK`.
    property var cur: []
    /// Newest published frame the ease is chasing.
    property var tgt: null
    /// PING-PONG buffers. Assigning a property a DIFFERENT array reference is
    /// what notifies QML; mutating one in place does not. Two persistent
    /// buffers give both — a new reference every pulse, and no allocation
    /// after the first two.
    readonly property var bufs: ({ a: [], b: [], flip: false })

    // The stash. `property var` because QML object state has no other home;
    // NOTHING binds to it, so writing it emits a notify nobody hears and —
    // the whole point — schedules no frame.
    property var pending: null

    visible: false
    width: 0
    height: 0

    // EASING — the transition between states, which is what reads as
    // "fluid", and which is NOT the frame rate.
    //
    // Measured 2026-08-19, and it falsified the standing assumption: driven at
    // QBZ_PULSE_MS=10 the shell presented ~100 frames/s (qt-batches confirmed
    // it) and the bars looked exactly as they had at 30. They would: each
    // published frame was applied as a STEP, so the extra presents repainted
    // the same discrete values three times over. What Slint does differently
    // is not rate, it is `animate bar-h { 90ms }` — it moves BETWEEN values.
    //
    // Restoring that costs NOTHING here, and that is the whole point. The old
    // interpolator was expensive because of its private 16 ms Timer, not
    // because of the interpolation: it presented at its own rate, unsynchronised
    // with everything else. Easing ON THE PULSE presents exactly as often as
    // stepping on the pulse did — one repaint per period either way. Only the
    // VALUE differs.
    //
    // The price is LAG, and it is the honest one: an exponential ease at
    // k = 0.45 is ~83% of the way there in 3 pulses (~100 ms), matching the
    // reference's 90 ms. Against the 46 ms FFT window and 77 ms of Rust
    // smoothing already in the chain, it is the same order as what is already
    // accepted.
    property real easeK: 0.45

    // Applied amplitude for bar `i`. Reads `cur` on every path — do NOT add an
    // early return, it would drop the dependency.
    function at(i) {
        var v = settle.cur[i] || 0;
        return v < 0 ? 0 : (v > 1 ? 1 : v);
    }

    onTargetChanged: {
        if (!settle.live) {
            // FROZEN — write NOTHING. The 2026-08-13 measurement that earns
            // this early return: an invisible consumer (the immersive panels
            // stay mounted while the overlay is closed) that writes from/to
            // on every publish still re-evaluates its delegates' geometry
            // bindings, and an invisible item's geometry change marks the
            // WHOLE WINDOW dirty — 28 bars x 30 Hz of phantom presents,
            // unsynchronised with the pulse (qt.quick.dirty named the nodes).
            // The last applied frame simply holds; the next live edge picks
            // up whatever publishes after it.
            return;
        }
        // Plain-JS copy, ONCE per publish (30/s), so `at()` indexes a JS array
        // instead of crossing into the QList_f32 sequence wrapper per bar.
        // The waveform arm ALREADY hands over a plain Array (it folds |sample|
        // in its own binding, SpectrumBand.qml:108-117), so skip the copy
        // there instead of paying a second 48-element pass per publish.
        var t = settle.target;
        var arr;
        if (Array.isArray(t)) {
            arr = t;
        } else {
            var n = t ? t.length : 0;
            arr = new Array(n);
            for (var i = 0; i < n; ++i)
                arr[i] = t[i];
        }
        settle.pending = arr;
    }

    onLiveChanged: {
        if (!settle.live) {
            settle.pending = null;
            settle.tgt = null;
        }
    }

    // THE driver: the shell's shared repaint pulse. Every consumer of this
    // edge (the ambient background drift included) dirties the scene in this
    // same event-loop turn, and the window presents once for all of them.
    // Writing nothing when there is no pending frame is what keeps a silent
    // or occluded shell at zero presents — the pulse NOTIFY alone never
    // schedules a repaint.
    Connections {
        target: QbzShell
        function onPulseMsChanged() {
            if (!settle.live)
                return;
            // Adopt the newest publish as the TARGET; the ease below is what
            // actually moves. A publish arriving mid-ease simply moves the
            // target — no from/to bookkeeping, no discontinuity.
            if (settle.pending !== null) {
                settle.tgt = settle.pending;
                settle.pending = null;
            }
            var t = settle.tgt;
            if (t === null)
                return;
            var n = t.length;
            if (n === 0)
                return;
            var src = settle.cur;
            var dst = settle.bufs.flip ? settle.bufs.a : settle.bufs.b;
            if (dst.length !== n)
                dst.length = n;
            var k = settle.easeK;
            var moved = false;
            for (var i = 0; i < n; ++i) {
                var a = src[i] || 0;
                var d = t[i] - a;
                // SELF-PARKING, preserved. Once the ease has converged the
                // values stop changing, this writes nothing and the pulse
                // schedules no repaint — the same zero-presents-when-silent
                // property the step version had. Without this guard a held
                // signal would repaint forever chasing a target it already
                // reached.
                if (d > 0.0005 || d < -0.0005)
                    moved = true;
                dst[i] = a + d * k;
            }
            if (!moved)
                return;
            settle.bufs.flip = !settle.bufs.flip;
            settle.cur = dst;
        }
    }
}
