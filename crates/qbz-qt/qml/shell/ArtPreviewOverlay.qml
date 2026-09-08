// Global floating album-art preview — a 220x220 cover that floats ABOVE the
// now-playing bar's small cover while the pointer hovers it. 1:1 port of
// ArtPreviewOverlay.slint.
//
// Two properties of the original are load-bearing and kept exactly:
//
//   1. It is mounted LAST in AppShell and fills the window, so it paints over
//      every surface without needing a z fight.
//   2. It is NON-INTERACTIVE — no MouseArea anywhere. The Slint tried a
//      PopupWindow first and it STOLE THE HOVER from the trigger (the cover's
//      has-hover flipped the moment the popup appeared, which closed it, which
//      restored the hover…), so the preview flickered. An overlay that cannot
//      receive events cannot do that; clicks also pass straight through to
//      whatever is underneath.
//
// The state (anchor + visibility) rides QbzShell, like the drag ghost in the
// same file — the trigger is inside the bar and the overlay is a sibling of
// the whole shell, so they cannot see each other's ids.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root
    anchors.fill: parent
    visible: QbzShell.artPreviewShow && QbzPlayer.npHasTrack

    QbzTheme { id: theme }

    // 232 = the 220 image plus a 6px frame on each side (ArtPreviewOverlay
    // .slint:16 `card`).
    readonly property int card: 232

    Rectangle {
        width: root.card
        height: root.card
        // Centred on the cover, clamped 8px inside the window, floating with
        // its bottom edge 8px above the cover's top.
        x: Math.max(8, Math.min(QbzShell.artPreviewX - width / 2,
                                Math.max(8, root.width - width - 8)))
        y: QbzShell.artPreviewY - height - 8
        radius: 8
        color: theme.surfaceCard
        border.width: 1
        border.color: theme.borderSubtle
        clip: true

        // The .slint carries a 16px drop shadow; QtQuick's DropShadow is a
        // shader effect and shader effects render NOTHING on this port's
        // software path (the phase-2 finding that also killed ColorOverlay in
        // QbzIcon), so the depth cue is a border instead of a fake blurred
        // rectangle that would be wrong on both paths.

        Image {
            x: 6
            y: 6
            width: 220
            height: 220
            fillMode: Image.PreserveAspectFit
            // D2 (contract 04 §4): the small feed is now the thumbnail
            // variant (150px), too small for a 220px preview — prefer the
            // size-resolved large feed, fall back to the small one. The
            // request below asks with 2x DPR headroom (440 -> the 600
            // bucket).
            source: QbzPlayer.npArtworkPathLarge !== ""
                ? QbzPlayer.npArtworkPathLarge : QbzPlayer.npArtworkPath
            asynchronous: true
            smooth: true
            mipmap: true
        }
    }

    // Ask for the large feed while the preview is up (bucketed in Rust —
    // repeated shows of the same track at the same size are no-ops).
    onVisibleChanged: if (visible) QbzPlayer.requestNpArtworkSize(440)
}
