// AmbientAccent — the album-palette accent LIFTED for legibility over the
// ambient/immersive backdrop (2026-08-31 immersive visual cleanup).
//
// Why this exists: `QbzShell.ambientAccent` is the +180° COMPLEMENT of the
// album's dominant hue (ambient_qt.rs lyrics_accent_color), and the
// ReactiveSplitPanel used to derive yet another complement locally — which is
// how a deep-blue cover ended up with an ORANGE waveform and seek bar. The
// owner's rule: chrome accents must come from the album's OWN palette, with
// enough contrast to survive the ambient wash.
//
// So this reads `ambientPrimary` (the dominant album hue itself, S 0.85
// L 0.58 for chromatic covers) and lifts its lightness into a band that reads
// over both the dark immersive glass and a colorful ambient field, keeping
// the hue. Achromatic covers (hslHue == -1) degrade to a plain light grey
// instead of inventing a hue.
//
// Usage (instantiate like QbzTheme, read `.value`):
//   AmbientAccent { id: ambientAccent }
//   color: ambientAccent.value

import QtQuick
import com.blitzfc.qbz

QtObject {
    id: root

    readonly property color source: QbzShell.ambientPrimary

    readonly property color value: root.source.hslSaturation < 0.05
        ? Qt.rgba(1.0, 1.0, 1.0, 0.92)
        : Qt.hsla(Math.max(0.0, root.source.hslHue),
                  Math.min(0.85, Math.max(0.45, root.source.hslSaturation)),
                  Math.min(0.80, Math.max(0.66, root.source.hslLightness)),
                  1.0)
}
