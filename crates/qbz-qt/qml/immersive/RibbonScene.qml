// Spectral Ribbon (immersive shader scene, mode 4) — the QML wrapper for
// the C++ `RibbonItem` (cxx/ribbon_item.*, the persistent-spectrogram
// QQuickRhiItem). Block A3 of the 2026-08-15 immersive-completion contract
// (spec 01 §2.4).
//
// Its own file (the LineBedScene.qml idiom) so the RHI item's contract is
// one screen of QML — and so the C++ type name only has to resolve when
// this file compiles, which the tier gate already ensures never happens
// where the RHI can't run it (the Loader in ShaderSceneLayer.qml activates
// only while `root.active && scene === 4`).
//
// THE PULSE LAW: the item repaints ONLY from `pulseTick()`, which
// ShaderSceneLayer calls on the shared shell pulse (QbzShell.pulseMs) with
// the ribbon frame it stashed on the viz tick. `frame` has no NOTIFY and
// nothing binds it.

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

RibbonItem {
    id: root

    // mirrorVertically: mirror on Vulkan/D3D, NOT on OpenGL and NOT on Metal.
    // This line used to read "Vulkan/Metal/D3D" — Metal had never been run and
    // was assumed to follow Vulkan because it is not OpenGL; on the Mac it
    // flipped. The empirical matrix, and why the rule stays a negation, are in
    // LineBedScene.qml.
    // The feedback passes are self-consistent in texture space; this only
    // affects the composite.
    // Metal sides with OpenGL (2026-08-19) — the full matrix and the reason
    // this stays a negation are in LineBedScene.qml.
    mirrorVertically: GraphicsInfo.api !== GraphicsInfo.OpenGL
                      && GraphicsInfo.api !== GraphicsInfo.Metal

    // Called by ShaderSceneLayer on the pulse edge ONLY. `f` is the stashed
    // 517-byte frame (QbzShaderScene.ribbonFrame, raw bytes) or null when
    // the viz drain published nothing new (player paused); `peak` is the
    // pack's energyHi (the real-time ceiling line). The repaint still runs
    // on null so the item tracks resizes at the same cadence the sibling
    // scenes tick their clocks.
    function pulseTick(f, peak) {
        if (f !== null)
            frame = f
        energyHi = peak
        update()
    }
}
