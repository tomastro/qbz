// QualityMini — the icon-only quality mark used on cards and track rows.
//
// Now a thin alias over QualityBadge's "iconOnly" mode instead of its own
// drawing. The port had grown two divergent takes on the same badge; the
// Tauri original (QualityBadgeStatic's iconOnly branch) is the reference for
// both, and it also brings the MP3 tier this replica never had.
//
// Keeping the `tier` API means the existing call sites did not have to change
// in the same commit. Prefer passing bitDepth / samplingRate / format when a
// row carries the raw numbers, and let the badge resolve the tier itself.

import QtQuick

QualityBadge {
    id: qualityMiniRoot
    property alias tier: qualityMiniRoot.tierOverride
    mode: "iconOnly"
    visible: tierOverride !== "" || bitDepth > 0 || samplingRate > 0 || format !== ""
}
