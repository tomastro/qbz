// Plasma (immersive shader scene, mode 1) — the QML wrapper for the C++
// `PlasmaItem` (cxx/plasma_item.*, the feedback-fluid QQuickRhiItem).
// Block A2 of the 2026-08-15 immersive-completion contract (spec 01 §2.1).
//
// Its own file (the LineBedScene.qml idiom) so the RHI item's contract is
// one screen of QML — and so the C++ type name only has to resolve when
// this file compiles, which the tier gate already ensures never happens
// where the RHI can't run it (the Loader in ShaderSceneLayer.qml activates
// only while `root.active && scene === 1`).
//
// THE PULSE LAW: the item repaints ONLY from `pulseTick()`, which
// ShaderSceneLayer calls on the shared shell pulse (QbzShell.pulseMs) with
// the applied pack it stashed on the viz tick. The properties have no
// NOTIFY and nothing binds them; the palette colors ride QbzShell's ambient
// triad (they change on track change, not per tick).

import QtQuick
import QtQuick.Window
import com.blitzfc.qbz

PlasmaItem {
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

    // Called by ShaderSceneLayer on the pulse edge ONLY. `p` carries the
    // applied pack values + the palette + the local scene clock; assigning
    // the MEMBER properties and repainting ONCE per pulse is the whole
    // cadence contract (the sibling ShaderEffect scenes tick their clocks
    // at the same edge).
    function pulseTick(p) {
        time = p.time
        // THE AC-COUPLED ONSET, deliberately in the `beat` slot.
        //
        // Plasma multiplies its splats and its rotation jolt by this, and the
        // raw envelope stops being a hit on dense material: it decays x0.88 per
        // 33 ms, so above ~3 hits/s it never falls far enough to come back as a
        // spike and settles into a shallow ripple around 0.7 — a splat that
        // should detonate becomes a steady glow. `beatAc` subtracts the local
        // density floor, so a hit reads as a hit whatever the tempo. The other
        // scenes keep the raw `beat`; only this one wants the contrast.
        beat = p.beatAc
        // The RAW transient stays available for the short, sharp flash — it
        // decays x0.85 and it is not floor-subtracted, so it is the crisper of
        // the two.
        transientAmp = p.transientAmp
        bandsLo = p.bandsLo
        bandsHi = p.bandsHi
        level = p.level
        levelSmooth = p.levelSmooth
        energyLo = p.energyLo
        energyHi = p.energyHi
        primary = p.primary
        secondary = p.secondary
        accent = p.accent
        update()
    }
}
