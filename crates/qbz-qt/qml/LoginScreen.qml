// QBZ login screen — QML port of crates/qbz-ui/ui/login/LoginScreen.slint.
//
// Centered 720px dark card: logo, wordmark, ToS checkbox gating the
// primary button, system-browser sign-in (NO webview, NO email/password),
// phase narration, sign-in error box, offline-connectivity callout (with
// captive-portal line), session-restore error box, "Start offline" link,
// legal disclaimer.
//
// All user-visible strings go through QbzSession.tr() with the EXACT msgids
// of the Slint @tr() calls so the existing .po translations apply.
// State comes from the QbzBridge singleton (Slint's LoginState +
// OfflineState globals); actions call the bridge invokables.

import QtQuick
import com.blitzfc.qbz
import "controls"
import "theme"

Rectangle {
    id: root
    property bool kioskHost: false
    color: theme.surfaceMain
    // Square window corners (phase 12: opaque window; the compositor owns
    // any rounding).
    // Set by the Loader (custom chrome drag/maximize); unused here.
    property var hostWindow: null

    // Slint: in-out property tos-accepted: true.
    property bool tosAccepted: true

    QbzTheme { id: theme }

    Flickable {
        id: loginScroll
        anchors.fill: parent
        contentWidth: width
        contentHeight: root.kioskHost ? Math.max(height, card.height + 32) : height
        interactive: root.kioskHost
        clip: root.kioskHost
        boundsBehavior: Flickable.StopAtBounds
        Item {
            width: loginScroll.width
            height: loginScroll.contentHeight
    // Faked drop shadow (blur 32, offset-y 8, #00000066): a translucent
    // black rounded rect behind the card.
    // SUPERSEDED (2026-07-29): the justification was "Qt5Compat DropShadow is
    // not assumed to be installed on the target", of a piece with the retired
    // "effects render nothing" doctrine. Both effect modules ARE installed here
    // and this port runs on the GPU (OpenGL RHI, measured); the old note came
    // from an offscreen session, which forces the software renderer by
    // definition — theme/RoundedImage.qml now DETECTS the software path with
    // `GraphicsInfo.api`. A real blurred shadow is a visual change owing its
    // own parity pass, so the fake stays for now.
    Rectangle {
        anchors.horizontalCenter: card.horizontalCenter
        y: card.y + 8
        width: card.width
        height: card.height
        radius: theme.radiusLg
        color: theme.cardShadow
        opacity: 0.5
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: root.kioskHost ? Math.min(720, root.width - 32) : 720
        height: cardColumn.implicitHeight + 2 * theme.cardPadding
        color: theme.surfaceCard
        radius: theme.radiusLg

        Column {
            id: cardColumn
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.margins: theme.cardPadding
            spacing: 0

            // --- Brand ------------------------------------------------
            Image {
                anchors.horizontalCenter: parent.horizontalCenter
                source: "assets/qbz-logo.png"
                width: root.kioskHost ? 64 : 140
                height: root.kioskHost ? 64 : 140
                fillMode: Image.PreserveAspectFit
            }
            Item { width: 1; height: theme.spacingSm }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("QBZ", QbzSession.trRev)
                color: theme.textPrimary
                font.pixelSize: theme.fontWordmark
                font.weight: theme.weightSemibold
                font.letterSpacing: 8
            }
            Item { width: 1; height: 2 }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: QbzSession.tr("QOBUZ™ PLAYER", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontSubtitle
                font.letterSpacing: 4
            }

            Item { width: 1; height: root.kioskHost ? 12 : theme.spacingXl }

            // --- Terms of Service row --------------------------------
            Row {
                spacing: theme.spacingSm

                // Minimal dark checkbox (Slint QbzCheckbox equivalent).
                Rectangle {
                    id: tosCheckbox
                    width: root.kioskHost ? 44 : 18
                    height: root.kioskHost ? 44 : 18
                    anchors.verticalCenter: parent.verticalCenter
                    radius: 4
                    color: root.tosAccepted ? theme.accent : "transparent"
                    border.color: theme.textMuted
                    border.width: root.tosAccepted ? 0 : 1.5

                    QbzIcon {
                        anchors.centerIn: parent
                        visible: root.tosAccepted
                        name: "check"
                        width: 12
                        height: 12
                        // On the accent fill -> the on-accent selector, same
                        // as the real controls/QbzCheckbox.qml this row
                        // hand-copies (primitives/QbzCheckbox.slint:24 says
                        // accent-text, and the selector returns accent-text
                        // on 34 of the 35 palettes; it only overrides where
                        // accent-text drops under 3:1 — rose-pine-dawn,
                        // 2.56:1). The hand-copy has to track the real
                        // control or the TOS check and the Settings checks
                        // diverge on exactly that theme.
                        tintName: theme.accentGlyphTint
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.tosAccepted = !root.tosAccepted
                    }
                }
                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    text: QbzSession.tr("I have read the", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: theme.fontBody
                }
                Text {
                    id: tosLink
                    anchors.verticalCenter: parent.verticalCenter
                    text: QbzSession.tr("Qobuz™ Terms of Service", QbzSession.trRev)
                    color: theme.accent
                    font.pixelSize: theme.fontBody
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzSession.openTos()
                    }
                }
            }

            Item { width: 1; height: root.kioskHost ? 12 : theme.spacingLg }

            // --- Sign in ---------------------------------------------
            // Opens the user's default web browser (no embedded webview).
            QbzPrimaryButton {
                id: signInButton
                btnHeight: root.kioskHost ? 64 : 48
                width: parent.width
                property bool canSignIn: root.tosAccepted && QbzSession.loginPhase === 0
                label: QbzSession.tr("Sign in with your browser", QbzSession.trRev)
                btnEnabled: signInButton.canSignIn
                onClicked: QbzSession.signInViaBrowser()
            }

            Item { width: 1; height: theme.spacingMd }

            // --- Browser-flow status ---------------------------------
            // The browser may open in the background without stealing
            // focus, and the code exchange takes a few seconds — the
            // screen must narrate instead of sitting inert.
            Column {
                visible: QbzSession.loginPhase === 1
                width: parent.width
                spacing: 4
                Text {
                    width: parent.width
                    text: QbzSession.tr("Continue in your web browser — QBZ is waiting for you to finish signing in.", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: theme.fontBody
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                }
                Text {
                    width: parent.width
                    text: QbzSession.tr("The browser may open in the background without taking focus — check your other windows.", QbzSession.trRev)
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                }
                Item { width: 1; height: 4 }
                Item {
                    anchors.horizontalCenter: parent.horizontalCenter
                    width: cancelText.implicitWidth
                    height: cancelText.implicitHeight + 3
                    Text {
                        id: cancelText
                        text: QbzSession.tr("Cancel", QbzSession.trRev)
                        color: cancelArea.containsMouse ? theme.accent : theme.textMuted
                        font.pixelSize: theme.fontLink
                    }
                    Rectangle {
                        y: cancelText.implicitHeight + 1
                        width: cancelText.implicitWidth
                        height: 1
                        color: cancelArea.containsMouse ? theme.accent : theme.textMuted
                    }
                    MouseArea {
                        id: cancelArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzSession.cancelLogin()
                    }
                }
            }
            Text {
                visible: QbzSession.loginPhase === 2
                width: parent.width
                text: QbzSession.tr("Signing you in…", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: theme.fontBody
                horizontalAlignment: Text.AlignHCenter
            }
            // Sign-in failure box (phase 0 only).
            Rectangle {
                visible: QbzSession.loginPhase === 0 && QbzSession.loginError !== ""
                width: parent.width
                height: signInErrorColumn.implicitHeight + 24
                color: theme.surfaceElevated
                radius: theme.radiusSm
                border.width: 1
                border.color: theme.borderSubtle
                Column {
                    id: signInErrorColumn
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 12
                    spacing: 4
                    Text {
                        width: parent.width
                        text: QbzSession.tr("Sign-in failed", QbzSession.trRev)
                        color: theme.textPrimary
                        font.pixelSize: theme.fontBody
                        font.weight: theme.weightMedium
                        horizontalAlignment: Text.AlignHCenter
                    }
                    Text {
                        width: parent.width
                        text: QbzSession.loginError
                        color: theme.textMuted
                        font.pixelSize: theme.fontLegal
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.WordWrap
                    }
                }
            }
            Item { width: 1; height: 10 }

            // --- Offline-boot callout (spec §4.1) --------------------
            // Points at the Start-offline link right below. TWO audiences, and
            // until 2026-08-22 it served only one of them:
            //
            //   returning user, no network   — the case it was written for.
            //   NEVER HAD AN ACCOUNT         — the case it was invisible for.
            //
            // The old gate was `hasPreviousSession && connectivity === 2`, and
            // `hasPreviousSession` is seeded from `load_last_user_id()` — None
            // on a fresh profile, and only ever set true by a successful Qobuz
            // login. So the one piece of UI that points at "you can use this
            // without an account" was structurally unreachable for exactly the
            // person who needs it. Verified by driving a fresh profile: the
            // capability works, the signpost never appears.
            Column {
                visible: QbzSession.connectivity === 2 || !QbzSession.hasPreviousSession
                width: parent.width
                spacing: 2
                Rectangle {
                    width: parent.width
                    height: offlineColumn.implicitHeight + 24
                    color: theme.surfaceElevated
                    radius: theme.radiusSm
                    border.width: 1
                    border.color: theme.borderSubtle
                    Column {
                        id: offlineColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 12
                        spacing: 4
                        Text {
                            width: parent.width
                            // Say the RIGHT thing to each audience. Telling a
                            // first-run user with working internet that they
                            // have "no internet connection" is simply false,
                            // and it is what the screen used to imply the
                            // moment this box became visible to them.
                            text: QbzSession.connectivity === 2
                                ? QbzSession.tr("No internet connection — you can start in offline mode with your local library and downloads", QbzSession.trRev)
                                : QbzSession.tr("No Qobuz account? You can still use QBZ as a player for your own music library", QbzSession.trRev)
                            color: theme.textSecondary
                            font.pixelSize: theme.fontBody
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                        }
                        Text {
                            // ALSO gated on connectivity: a captive-portal line
                            // under a "No Qobuz account?" heading would be a
                            // non-sequitur for a first-run user who is online.
                            visible: QbzSession.captivePortal && QbzSession.connectivity === 2
                            width: parent.width
                            text: QbzSession.tr("A network sign-in page may be blocking the connection", QbzSession.trRev)
                            color: theme.textMuted
                            font.pixelSize: theme.fontLegal
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                        }
                    }
                }
                // Caret pointing at the Start-offline link below.
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: "▼"
                    color: theme.textMuted
                    font.pixelSize: theme.fontLegal
                }
            }

            // Init-error variant of the same box (session restore failed
            // for a non-connectivity reason). Hidden while the
            // connectivity box shows — never two boxes at once.
            Column {
                visible: QbzSession.restoreError !== "" && QbzSession.connectivity !== 2
                width: parent.width
                spacing: 0
                Rectangle {
                    width: parent.width
                    height: restoreColumn.implicitHeight + 24
                    color: theme.surfaceElevated
                    radius: theme.radiusSm
                    border.width: 1
                    border.color: theme.borderSubtle
                    Column {
                        id: restoreColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: 12
                        spacing: 4
                        Text {
                            width: parent.width
                            text: QbzSession.tr("Could not restore your session", QbzSession.trRev)
                            color: theme.textPrimary
                            font.pixelSize: theme.fontBody
                            font.weight: theme.weightMedium
                            horizontalAlignment: Text.AlignHCenter
                        }
                        Text {
                            width: parent.width
                            text: QbzSession.restoreError
                            color: theme.textMuted
                            font.pixelSize: theme.fontLegal
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.WordWrap
                        }
                    }
                }
                Item { width: 1; height: theme.spacingSm }
            }

            // Always visible (#553): without a previous session this
            // opens the GUEST profile (user 0) — Local Library only,
            // adopted by the account on the first real login.
            Item {
                id: offlineButton
                anchors.horizontalCenter: parent.horizontalCenter
                width: offlineText.implicitWidth
                height: root.kioskHost ? 64 : offlineText.implicitHeight + 3
                activeFocusOnTab: true
                Accessible.role: Accessible.Button
                Accessible.name: offlineText.text
                Accessible.onPressAction: QbzSession.startOffline()
                Keys.onPressed: function (event) {
                    if (!event.isAutoRepeat
                            && (event.key === Qt.Key_Return
                                || event.key === Qt.Key_Enter
                                || event.key === Qt.Key_Space)) {
                        QbzSession.startOffline()
                        event.accepted = true
                    }
                }
                Text {
                    id: offlineText
                    text: QbzSession.tr("Start offline (no access to Qobuz™ services)", QbzSession.trRev)
                    color: offlineArea.containsMouse || offlineButton.activeFocus
                           ? theme.accent : theme.textMuted
                    font.pixelSize: theme.fontLink
                }
                Rectangle {
                    y: offlineText.implicitHeight + 1
                    width: offlineText.implicitWidth
                    height: 1
                    color: offlineArea.containsMouse || offlineButton.activeFocus
                           ? theme.accent : theme.textMuted
                }
                MouseArea {
                    id: offlineArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onPressed: offlineButton.forceActiveFocus()
                    onClicked: QbzSession.startOffline()
                }
            }

            Item { width: 1; height: root.kioskHost ? 12 : theme.spacingXl }

            // --- Legal disclaimer ------------------------------------
            Text {
                width: parent.width
                text: QbzSession.tr("QBZ requires a Qobuz™ account. Without an active subscription, playback is limited to 30-second previews; favorites, playlists and purchases still work. Your credentials are sent directly to Qobuz™.", QbzSession.trRev)
                    + "\n"
                    + QbzSession.tr("QBZ can be used as an offline player without a Qobuz™ account (no access to the Qobuz™ library).", QbzSession.trRev)
                    + "\n"
                    + QbzSession.tr("This application uses the Qobuz API but is not certified by Qobuz.", QbzSession.trRev)
                    + " "
                    + QbzSession.tr("Qobuz™ is a trademark of Qobuz. QBZ is an open-source application licensed under the MIT License and is not affiliated with, endorsed by, or certified by Qobuz.", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }
        }
    }
        }
    }
}
