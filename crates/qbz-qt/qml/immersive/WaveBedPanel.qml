// WaveBedPanel — FOCUS mode 6, the full-screen VERTICAL-mirror waveform
// (§6.1 of the 2026-08-02 immersive-port contract), port of
// crates/qbz-ui/ui/immersive/ImmersiveWaveBedPanel.slint:24-74.
//
// OPAQUE #000 (:49) — hides the atmosphere, intended (§5.1). This is NOT the
// Spectrum panel: every column is centered on the viewport Y midline and
// grows up AND down symmetrically (the full-screen twin of the dock's
// WaveCol, :5-13).
//
// GEOMETRY (:52-72): 48 downsampled L-channel columns (waveform indices
// 0..235 step 5 — the L half is samples 0..255), slot = vw/48, bar =
// slot-4, x = i*slot + 2, colH = max(2, amp*0.7*vh) with amp = min(1, |w|),
// y = (H - h)/2 (the vertical mirror), radius 1, color
// QbzShell.ambientPrimary (the Slint ImmersiveState.spectrum-primary — same
// album-derived pipeline, SpectrumBand.qml:109-113 precedent).
//
// FFT CONSUMPTION (trap 6): ONE VizSettle on QbzViz.waveform; the
// |sample|-fold + downsample happens ONCE per publish in the settle's
// `target` binding (48 ops — the SpectrumBand.qml:239-248 idiom), never per
// column per frame. The inter-frame settle is VizSettle's (the Slint
// `animate col-h 70ms` equivalent, :31).
//
// The bottom-right ImmersiveSongCard is mounted by ImmersiveView (layer 4),
// NOT inside this panel (:15-16).

import QtQuick
import com.blitzfc.qbz
import "../shell"

Rectangle {
    id: root

    color: "#000000"

    readonly property real slot: root.width / 48
    // Max half-amplitude reach ~0.35*H up and down (full bar = 0.7*H tall).
    readonly property real maxColH: root.height * 0.7

    // ONE VizSettle for the whole stream (trap 6). The |sample| envelope is
    // folded HERE, once per publish, instead of an abs+min per column per
    // rendered frame (VizSettle branches on Array.isArray and skips its own
    // copy for this arm).
    VizSettle {
        id: waveSettle
        live: root.visible && QbzImmersive.open
        target: {
            var s = QbzViz.waveform
            var out = new Array(48)
            for (var i = 0; i < 48; ++i) {
                var v = s[i * 5] || 0
                if (v < 0) v = -v
                out[i] = v > 1 ? 1 : v
            }
            return out
        }
    }

    // STATIC Repeater model (48 columns); only each column's HEIGHT binds to
    // the stream.
    Repeater {
        model: 48
        delegate: Rectangle {
            required property int index
            x: index * root.slot + 2
            width: root.slot - 4
            height: Math.max(2, waveSettle.at(index) * root.maxColH)
            // Centered on the viewport Y-mid -> vertical mirror (:37-38).
            anchors.verticalCenter: parent.verticalCenter
            radius: 1
            color: QbzShell.ambientPrimary
        }
    }
}
