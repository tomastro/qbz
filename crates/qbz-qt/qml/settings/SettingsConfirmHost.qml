// One confirm surface for the whole Settings view.
//
// WHY IT LIVES AT THE VIEW ROOT AND NOT IN THE PANELS: every section is a
// Column inside SettingsView's Flickable, so a modal mounted in a panel is
// sized by the scrolled content and clipped by the viewport — it would ride
// the scroll instead of covering the window. This mounts once, last, at the
// view root (over the sub-nav too) and the panels are handed a reference to
// it beside their `doc`.
//
// The reference raises NATIVE rfd message boxes for these
// (local_library_settings.rs:607-626, plex_auth.rs:963/997). This port has an
// in-app confirm surface already — controls/QbzConfirmModal.qml — so the
// prompts stay inside the window, but the STRINGS and the STEP COUNT are 1:1
// with the reference: the danger-zone clear asks twice, Plex asks once.
//
// Two separate modal instances rather than one reused instance: chaining
// would mean rewriting `title`/`body` from inside the first modal's own
// `confirmed` handler, and the two prompts also want independent focus
// lifetimes (QbzConfirmModal grabs focus on open and restores the shell root
// on close). Cheap enough — they are inert Items while closed.

import QtQuick
import com.blitzfc.qbz
import "../controls"

Item {
    property bool kioskHost: false

    id: root

    anchors.fill: parent
    // Above the settings content; QbzConfirmModal itself also declares 3100
    // (ADR-009 wants >= 3000).
    z: 3100
    // Inert while nothing is being asked, so it never eats a press meant for
    // the panel underneath.
    visible: step1.opened || step2.opened
    enabled: root.visible

    /// Single-step prompt. `cb` runs only on confirm.
    function ask(title, body, confirmLabel, cb) {
        step1.title = title
        step1.body = body
        step1.confirmLabel = confirmLabel
        step1.secondTitle = ""
        step1.pending = cb
        step1.open()
    }

    /// Two-step prompt — the reference's danger-zone shape: the second
    /// question is only asked once the first is accepted, and `cb` runs only
    /// after BOTH.
    function askTwice(t1, b1, c1, t2, b2, c2, cb) {
        step1.title = t1
        step1.body = b1
        step1.confirmLabel = c1
        step1.secondTitle = t2
        step1.secondBody = b2
        step1.secondConfirm = c2
        step1.pending = cb
        step1.open()
    }

    QbzConfirmModal {
        kioskHost: root.kioskHost
        id: step1
        anchors.fill: parent
        danger: true
        /// Non-empty means "ask a second time before running `pending`".
        property string secondTitle: ""
        property string secondBody: ""
        property string secondConfirm: ""
        /// The callback, held between open and confirm. Cleared on every exit
        /// path so a cancelled prompt can never fire a stale action later.
        property var pending: null

        onConfirmed: {
            if (step1.secondTitle !== "") {
                step2.title = step1.secondTitle
                step2.body = step1.secondBody
                step2.confirmLabel = step1.secondConfirm
                step2.pending = step1.pending
                step1.pending = null
                step2.open()
                return
            }
            const cb = step1.pending
            step1.pending = null
            if (cb)
                cb()
        }
        onCancelled: step1.pending = null
    }

    QbzConfirmModal {
        kioskHost: root.kioskHost
        id: step2
        anchors.fill: parent
        danger: true
        property var pending: null
        onConfirmed: {
            const cb = step2.pending
            step2.pending = null
            if (cb)
                cb()
        }
        onCancelled: step2.pending = null
    }
}
