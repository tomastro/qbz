// The Windows "as-is" disclaimer, shown once per version at startup.
//
// Structure, scrim, shadow, z and the self-gate follow WhatsNewModal.qml,
// which follows LogViewerModal.qml. Colour literals are `Qt.rgba(...)`
// because Slint is #RRGGBBAA and Qt is #AARRGGBB -- the copy that shipped an
// invisible scrim in five modals, fixed 2026-08-14.
//
// ── WHY THE BODY IS NOT TRANSLATED ─────────────────────────────────────────
//
// This is the author's own statement of policy, quoted from the release notes
// ("Windows -- yes, but read this first"). The wording is the point: it says
// what is and is not promised, and in whose voice. The CHROME around it (the
// title, the checkbox, the button) goes through `tr` like everything else, so
// the frame localises and the statement does not get paraphrased by a
// translator into something that promises more or less than it does.
//
// ── WHY THIS IS NOT `Text.MarkdownText` ────────────────────────────────────
//
// Same reason WhatsNewModal gives, minus the parser: the source has exactly
// two constructs, `**bold**` and one link, so the paragraphs below carry
// `<b>` and `<a>` and render as StyledText. No dependency, no subset to
// disagree about.

import QtQuick
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root

    anchors.fill: parent
    z: 3000

    QbzTheme { id: theme }

    // The body, verbatim from the release notes. One entry per paragraph.
    // `lead: true` renders at the emphasis size -- the two sentences that are
    // the actual disclaimer rather than the explanation around it.
    readonly property var paragraphs: [
        { lead: false, text: "There is now a Win32 build of QBZ." },
        { lead: false, text: "There is, however, one important distinction:" },
        { lead: true,  text: "<b>Windows is not a supported QBZ platform.</b>" },
        { lead: false, text: "The Windows build is provided <b>as-is</b>. Think of it like buying something second-hand: what you see is what you get." },
        { lead: false, text: "I won't promise feature parity, chase Windows-specific regressions, maintain a separate Windows roadmap, or guarantee that something Microsoft changes six months from now won't break it." },
        { lead: false, text: "If it works for you, great. If it doesn't, there is no promise that I'll fix it." },
        { lead: false, text: "And to be completely transparent about why: <b>I hated using Windows while working on this port.</b>" },
        { lead: false, text: "Installing it, configuring the build box, dealing with the environment and especially figuring out its audio setup reminded me why I don't use it in the first place. I have no desire to spend more time than absolutely necessary inside Redmond's OS every time QBZ gets a release." },
        { lead: false, text: "Sorry. Long-time Gentoo Linux user. Some biases are too old to fix." },
        { lead: false, text: "So why ship it?" },
        { lead: false, text: "Because the hard part — getting QBZ running properly on Win32 — is done, and useful code is better in people's hands than sitting in a branch because I personally don't want to maintain another operating system." },
        { lead: false, text: "More importantly, <b>the Windows port is open for adoption</b>." },
        { lead: false, text: "That's essentially what happened with macOS. <a href=\"https://github.com/afonsojramos\">@afonsojramos</a> took ownership of the macOS port and is the reason it grew from an experiment into a properly supported QBZ platform." },
        { lead: false, text: "If someone wants to do the same for Windows, maintain it and eventually remove this rather large disclaimer, get in touch." }
    ]

    property bool remember: false

    // Component.onCompleted, NOT onVisibleChanged. `Item.visible` defaults to
    // true and the Loader only creates this when it should already be shown,
    // so there is no false->true transition to observe: the old handler never
    // ran, focus was never taken, Escape never reached the handler below, and
    // ordinary hotkeys kept working BEHIND the modal.
    Component.onCompleted: keyScope.forceActiveFocus()

    // Qt strands activeFocus on an item that is being destroyed, which kills
    // AppShell's key dispatcher until the next click. Hand focus back to the
    // shell root on the way out -- the same duck-walk QbzConfirmModal uses,
    // and for the same measured reason.
    Component.onDestruction: root._restoreShellFocus()

    function _restoreShellFocus() {
        var p = root
        while (p.parent) {
            if (p.parent.isQbzShellRoot === true) {
                p.parent.forceActiveFocus()
                return
            }
            p = p.parent
        }
    }

    FocusScope {
        id: keyScope
        anchors.fill: parent
        // Escape closes WITHOUT remembering: dismissing a disclaimer by
        // reflex must never be read as having accepted it for good.
        Keys.onEscapePressed: QbzShell.dismissWindowsDisclaimer(false)
    }

    Rectangle {
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.75)
        // No click-through close. Unlike the other modals this one is the
        // first thing a new user sees and says what is not promised; a stray
        // click on the scrim should not dismiss it.
        MouseArea {
            anchors.fill: parent
            // Wheel-lock (the DiscoverConfigModal rule).
            onWheel: function (wheel) { wheel.accepted = true }
        }
    }

    Rectangle {
        anchors.centerIn: panel
        width: panel.width + 8
        height: panel.height + 8
        radius: theme.radiusMd
        color: Qt.rgba(0, 0, 0, 0.5)
    }

    Rectangle {
        id: panel
        anchors.centerIn: parent
        width: Math.min(root.width - 80, 720)
        height: Math.min(root.height - 80, 640)
        radius: theme.radiusMd
        color: theme.surfaceCard
        border.width: 1
        border.color: theme.borderSubtle

        MouseArea {
            anchors.fill: parent
            // Wheel-lock (the DiscoverConfigModal rule).
            onWheel: function (wheel) { wheel.accepted = true }
        }

        Item {
            anchors.fill: parent
            anchors.margins: 24

            Text {
                id: headerText
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                text: QbzSession.tr("Windows — yes, but read this first", QbzSession.trRev)
                color: theme.textPrimary
                font.pixelSize: 18
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Flickable {
                id: scroller
                anchors.top: headerText.bottom
                anchors.topMargin: 16
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.bottom: footerRow.top
                anchors.bottomMargin: 16
                clip: true
                contentWidth: width
                contentHeight: body.height
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: body
                    width: scroller.width
                    spacing: 12

                    Repeater {
                        model: root.paragraphs
                        Text {
                            width: parent.width
                            text: modelData.text
                            // StyledText, not RichText: it is the cheap
                            // subset and it is all these paragraphs use.
                            textFormat: Text.StyledText
                            wrapMode: Text.WordWrap
                            color: modelData.lead ? theme.textPrimary : theme.textSecondary
                            font.pixelSize: modelData.lead ? 15 : 13
                            linkColor: theme.accent
                            onLinkActivated: function (url) { Qt.openUrlExternally(url) }
                        }
                    }
                }
            }

            Item {
                id: footerRow
                anchors.bottom: parent.bottom
                anchors.left: parent.left
                anchors.right: parent.right
                height: 32

                Row {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 8

                    QbzCheckbox {
                        id: rememberBox
                        anchors.verticalCenter: parent.verticalCenter
                        checked: root.remember
                        // `toggled()` carries NO argument here (see
                        // QbzCheckbox.qml): the control reports that it was
                        // hit, and the owner of the state flips it.
                        onToggled: root.remember = !root.remember
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: QbzSession.tr("Don't show this again", QbzSession.trRev)
                        color: theme.textSecondary
                        font.pixelSize: 13
                        MouseArea {
                            anchors.fill: parent
                            onClicked: root.remember = !root.remember
                        }
                    }
                }

                QbzPrimaryButton {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    // `label`, not `text` -- this control is not a Qt Button.
                    label: QbzSession.tr("I understand", QbzSession.trRev)
                    btnHeight: 32
                    onClicked: QbzShell.dismissWindowsDisclaimer(root.remember)
                }
            }
        }
    }
}
