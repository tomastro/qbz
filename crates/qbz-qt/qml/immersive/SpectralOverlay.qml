// Spectral Ribbon overlay (immersive scene mode 4) — green axes + labels +
// the stream header, drawn OVER the RibbonItem's GPU spectrogram. QML port
// of qbz-ui's ImmersiveSpectralOverlay.slint (B-block idiom: everything is
// positioned as FRACTIONS of the viewport so the axes grow/shrink with the
// window). The plot rectangle MUST match spectral_ribbon.frag
// (PLOT_X0..Y1); the overlay fractions are the Slint TOP-DOWN ones
// (0.070/0.780 = 1 - the shader's 0.930/0.220).
//
// No MouseArea anywhere — input passes through to the player bar below
// (the Slint overlay's "plain Rectangle, no TouchArea" rule).

import QtQuick
import com.blitzfc.qbz

Item {
    id: root
    anchors.fill: parent

    // Plot rect — the Slint overlay's fractions (see the header).
    readonly property real plotX0: width * 0.055
    readonly property real plotX1: width * 0.970
    readonly property real plotY0: height * 0.070
    readonly property real plotY1: height * 0.780
    readonly property real plotW: plotX1 - plotX0
    readonly property real plotH: plotY1 - plotY0

    readonly property color axis: "#5fb87a"
    // Nyquist in kHz (delivered rate / 2). Falls back to 22.05 (44.1k)
    // when nothing is reported yet (npEffRateHz 0).
    readonly property real nyqKhz: QbzPlayer.npEffRateHz > 0
        ? QbzPlayer.npEffRateHz / 2000.0
        : 22.05
    readonly property int dur: QbzPlayer.npDurationSecs

    // --- Stream header (top center, green box) ---------------------------
    Rectangle {
        // The immersive search cortinilla lives at the same height in the
        // header row — while the search is open it wins the space (owner
        // smoke 2026-08-15: they overlapped).
        visible: !QbzImmersive.immSearchOpen
        x: (root.width - width) / 2
        y: root.height * 0.045
        width: hdr.implicitWidth + 28
        height: 22
        radius: 5
        border.width: 1
        border.color: root.axis
        color: "#e60a140d"
        Text {
            id: hdr
            anchors.centerIn: parent
            text: "Stream 1/1: Audio Stream, " + QbzPlayer.npEffRateHz + " Hz, "
                + QbzPlayer.npEffBits + " bits, FFT:4096, Bands:512"
            color: root.axis
            font.pixelSize: 12
        }
    }

    // --- Axis frame (L-shape) --------------------------------------------
    Rectangle {  // vertical (frequency)
        x: root.plotX0
        y: root.plotY0
        width: 1
        height: root.plotH
        color: root.axis
    }
    Rectangle {  // horizontal (time)
        x: root.plotX0
        y: root.plotY1
        width: root.plotW
        height: 1
        color: root.axis
    }

    // --- Frequency axis labels (Y, left) — 0 .. Nyquist ------------------
    Repeater {
        model: [0.0, 0.25, 0.5, 0.75, 1.0]
        Text {
            required property real modelData
            x: root.plotX0 - 46
            y: root.plotY1 - modelData * root.plotH - height / 2
            width: 40
            horizontalAlignment: Text.AlignRight
            text: Math.round(root.nyqKhz * modelData) + "k"
            color: root.axis
            font.pixelSize: 11
        }
    }

    // --- Time axis labels (X, bottom) — 30 s ticks -----------------------
    Repeater {
        model: 31
        Text {
            required property int index
            visible: root.dur > 0 && (index * 30) <= root.dur
            x: root.plotX0 + (root.dur > 0 ? root.plotW * ((index * 30) / root.dur) : 0) - width / 2
            y: root.plotY1 + 6
            text: Math.floor((index * 30) / 60) + ":"
                + ((index * 30) % 60 < 10 ? "0" : "") + ((index * 30) % 60)
            color: root.axis
            font.pixelSize: 10
        }
    }
    // Final duration marker at the right edge.
    Text {
        visible: root.dur > 0
        x: root.plotX1 - width / 2
        y: root.plotY1 + 6
        text: Math.floor(root.dur / 60) + ":"
            + (root.dur % 60 < 10 ? "0" : "") + (root.dur % 60)
        color: root.axis
        font.pixelSize: 10
    }
}
