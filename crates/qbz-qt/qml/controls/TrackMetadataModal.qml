// Per-track entry point for the shared metadata workspace. It deliberately
// does not own a second form: every album/track field, provider lookup,
// artwork action, validation rule and persistence choice comes from
// TagEditorModal. The only additions are dialog chrome, sibling navigation
// and promotion to the routed full-album view.

import QtQuick
import com.blitzfc.qbz

Item {
    id: root
    visible: QbzTagEditor.trackEditorOpen
    enabled: visible

    TagEditorModal {
        anchors.fill: parent
        trackMode: true
        // Promotion destroys this wrapper while preserving Rust's immutable
        // session for the full-page instance mounted by ContentRouter.
        leaveOnDestruction: false
    }
}
