// Musician modal — QML port of Tauri's MusicianModal.svelte (322 lines, read in
// full). The surface a WEAK / NONE / resolve-error musician click lands on: it
// is NOT a page. Its own docblock is accurate — "provides context only", "does
// NOT show albums, tracks, or playlists", "does NOT allow deep navigation" —
// and it is the majority case, because most sidemen are not Qobuz artists.
//
// THE PAIR: this and views/MusicianPageView.qml COEXIST. A single dispatcher in
// Rust (musician_qt.rs) picks between them by confidence and publishes either
// `musicianJson` (page) or `modalJson` (this). QML never branches on
// confidence — 00-CONTRACT §13.2.
//
// MOUNT: a global overlay in the AppShell tree, `anchors.fill: parent`, the
// same self-gated shape KeyboardShortcutsModal / CustomizeShortcutsModal use.
// While `modalJson` is "" this is an invisible, non-interactive Item that
// parses nothing. z:3200 satisfies ADR-009 (>= 3000) and clears the port's
// in-pane modals at 3100, so it also paints above the immersive overlay — which
// is the required behaviour for consumer #6: a weak/none click inside the
// immersive Track-info panel must open THIS above the still-open overlay, while
// a contextual one navigates underneath it (00-CONTRACT §11/O6).
//
// Anatomy, every number from the .svelte:
//   :129-139  backdrop rgba(0,0,0,0.7) full-window, click-outside dismisses
//   :146-155  card surface-card, radius 16, width 90% max 420, max-height 85vh,
//             its own scroller, shadow 0/24/48 rgba(0,0,0,0.4)
//   :168-174  header padding 24/24/16, align-items flex-start, 1px bottom rule
//   :182-192  48px circular icon, surface-elevated, User glyph 24 muted
//             (the page's is 56/32 — the modal is one step smaller)
//   :200-212  name h2 20/600 lh 1.2, role 14 muted CAPITALIZED
//   :214-232  close 32x32 radius 8 surface-elevated, muted -> hover bg-hover +
//             text-primary, X glyph 18
//   :234-253  content padding 24; section margin-bottom 20; h3 11/600 UPPERCASE
//             letter-spacing 0.5 muted, margin-bottom 12
//   :255-272  "Known for" — a VERTICAL list (the page uses horizontal chips),
//             gap 8, rows 14 primary with a 14px muted Music glyph
//   :274-298  info box: row gap 12, padding 16, surface-elevated, radius 12;
//             Info glyph 16 muted nudged 2px down; copy 13 / line-height 1.5
//   :300-321  the CTA — see UNREACHABLE below
//
// LIVE GEOMETRY, which reading those three margins in isolation gets wrong
// (08-critique-findings, MINOR on §4.3): `bands` is empty in practice, so the
// info section is BOTH the first and the last child of .modal-content. Its
// 24px margin-top does not collapse against the container's 24px padding, so
// the box sits 48px below the header rule and 0px above the card bottom (a
// last-child margin-bottom of 0, then the container's own 24px padding). A port
// that places it at 24 draws a visibly shorter, top-heavy modal. Hence the
// content Column below is padded 24 and the info block carries its own 24.
//
// DELIBERATE DIVERGENCES
//  - DV-12/§7   the three explanatory strings are hardcoded English in Tauri
//               and translated here. The catalogue's msgid IS the English
//               source string, so the English output is byte-identical and the
//               1:1 is fully preserved; the other seven locales gain a real
//               translation. Same for the close tooltip.
//  - ADR-008    Tauri's band rows are plain text with no container, so this
//               surface has no pill to demote — unlike the page's chips (DV-19).
//  - no entry animation. Tauri fades the backdrop in over 150ms and slides the
//               card up 20px over 200ms. No modal in this port animates its
//               entry (KeyboardShortcutsModal, CustomizeShortcutsModal,
//               QbzConfirmModal, AlbumInfoModal, TrackInfoModal — none), and
//               matching them keeps one behaviour across the shell. Recorded so
//               it is a decision, not an omission.
//  - no backdrop blur. `backdrop-filter: blur(4px)` needs a live blur of
//               everything behind the scrim; the port's shadow/blur doctrine
//               (TrackInfoModal.qml:17-25) is that such a change owes its own
//               parity pass rather than riding along with a port.
//  - the shadow is FAKED (a translucent rounded rect behind the card), the
//               LoginScreen / QbzConfirmModal / KeyboardShortcutsModal
//               precedent, for the same reason.
//  - `user-select: text` on the card (app.css:1580-1584) has no cheap QML
//               equivalent — a selectable Text is a TextEdit — so the copy is
//               not selectable here. Flagged, not silently dropped.
//
// COLOURS: tokens only, per 00-CONTRACT §4.3. The two rgba() literals are
// written as Qt.rgba(r,g,b,a) with 0..1 floats and NEVER as 8-digit hex: CSS
// and Slint spell it #RRGGBBAA, Qt parses #AARRGGBB, so `#00000066` reads as
// FULLY TRANSPARENT black here. That trap has already landed twice in this
// port. The two 1px rules bind `borderSubtle` rather than `surfaceElevated`
// (Tauri styles them with --bg-tertiary, which is the same literal on dark):
// a divider's ROLE token is the border one, and using a surface fill as a
// border colour is what breaks in the light themes.
//
// ESCAPE / FOCUS (00-CONTRACT §5.2). Tauri dismisses on Escape, backdrop click
// and the X. Qt's central Escape stack (hotkeys_qt.rs:629-683,
// hotkeys_bridge.rs:370-401) does not know this modal and CONSUMES Escape even
// when it closes nothing, so Escape had to be owned explicitly. It is owned the
// way every other modal in this tree owns it, and no Rust registration is
// needed: an Item/FocusScope modal's Keys.onEscapePressed fires BEFORE the
// central stack and accepts the event, because the scope holds active focus and
// Qt walks the focus chain up to the AppShell root dispatcher, not down
// (KeyboardShortcutsModal.qml:43-51, "trap 4"). The grab is DEFERRED 30ms past
// the open edge — focusing synchronously races the visibility transition, and
// this modal is especially exposed because it opens asynchronously right after
// AlbumInfo or TrackInfo closes. On the close edge Qt strands activeFocus on
// the now-invisible scope, which kills the AppShell dispatcher until the next
// click, so every close path hands focus back to the shell root by duck-walking
// to `isQbzShellRoot` (QbzConfirmModal.qml:85-94). The restore is on the EDGE,
// not inside close(), so a close driven from Rust (the dispatcher opening the
// page instead, a language switch, a shutdown) restores too.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // "" = closed. The document IS the open flag (00-CONTRACT §13.2), so there
    // is no second bridge property to keep in sync. `musicianRev` is a pure
    // binding dependency, the QbzSession.tr(msgid, rev) idiom: the parse
    // ignores it, reading it is what re-runs this binding when Rust republishes
    // a byte-identical document.
    readonly property string raw: QbzMusician.modalJson || ""
    readonly property bool opened: root.raw !== ""
    readonly property var doc: root._parse(root.raw, QbzMusician.musicianRev)
    function _parse(s, rev) {
        try {
            return JSON.parse(s || "{}")
        } catch (e) {
            return ({})
        }
    }

    readonly property var bands: doc.bands || []

    // CSS `text-transform: capitalize` — title-case every WORD and leave the
    // rest of it alone. QML has no text-transform; see MusicianPageView.qml.
    function _capitalize(s) {
        if (!s)
            return ""
        return String(s).replace(/(^|\s)(\S)/g, function (m, sp, ch) {
            return sp + ch.toUpperCase()
        })
    }

    // `explanatoryCopy` (:27-36). Three arms, and the third is the `default:`
    // one — it fires for ANY confidence the switch does not name, so it is not
    // dead the way the CTA is.
    readonly property string explanatoryCopy: {
        var c = root.doc.confidence || ""
        if (c === "weak")
            return root.t("This musician appears in album credits but has limited catalog information available in Qobuz.")
        if (c === "none")
            return root.t("Unable to verify this musician's catalog. The information shown is based on album credits.")
        return root.t("This musician appears in album credits.")
    }

    // UNREACHABLE BY CONSTRUCTION, and present anyway (:39-41, :117-123). The
    // dispatcher only opens this modal for `confirmed` when the Qobuz id is
    // absent or zero, so the CTA's own guard can never pass; Tauri is identical.
    // `qobuzArtistId` is NOT a member of the frozen `modalJson` shape
    // (00-CONTRACT §13.2), so this reads `undefined` and the gate is false —
    // which is exactly the wanted behaviour, and lights the block up unchanged
    // the day the Rust lane adds the field. Reading an absent key off a parsed
    // JSON object is not an error in QML, and it is not a bridge member, so
    // qml_singleton_xref has nothing to complain about.
    readonly property bool showArtistCta: root.doc.confidence === "confirmed"
        && (root.doc.qobuzArtistId || 0) > 0

    visible: root.opened
    enabled: root.opened
    // ADR-009 (>= 3000), above the port's in-pane modals at 3100 and above the
    // immersive overlay — see the header note on consumer #6.
    z: 3200

    function _close() {
        QbzMusician.closeModal()
    }

    // :43-48 — navigate, THEN close, in that order (Tauri's own ordering, and
    // the same order §2.2 records for the two parent modals).
    function _openArtist() {
        if (!root.showArtistCta)
            return
        QbzArtist.openArtist(String(root.doc.qobuzArtistId))
        root._close()
    }

    // Focus lifecycle on the OPEN/CLOSE EDGE — see the header.
    onOpenedChanged: {
        if (root.opened) {
            focusGrab.restart()
        } else {
            var p = root
            while (p.parent) {
                if (p.parent.isQbzShellRoot === true) {
                    p.parent.forceActiveFocus()
                    return
                }
                p = p.parent
            }
        }
    }

    Timer {
        id: focusGrab
        interval: 30
        repeat: false
        onTriggered: keyScope.forceActiveFocus()
    }

    // The keyboard focus ring — the modal is keyboard-operable for the same
    // reason the page is (§5.2): every Tauri control here is a native <button>.
    component FocusRing: Rectangle {
        property Item host: null
        property real inset: 3
        anchors.fill: parent
        anchors.margins: -inset
        color: "transparent"
        border.width: 2
        border.color: theme.focusRing
        visible: host !== null && host.activeFocus
        z: 5
    }

    FocusScope {
        id: keyScope
        anchors.fill: parent

        // The modal's OWN Escape. Accepts, so the central §1.2 stack never
        // double-fires on the same press.
        Keys.onEscapePressed: function (event) {
            root._close()
            event.accepted = true
        }

        // Backdrop — rgba(0, 0, 0, 0.7). A click ON THE BACKDROP dismisses;
        // Tauri gates that on `e.target === e.currentTarget` so a click that
        // bubbled out of the card never closes. Here the card's own MouseArea
        // swallows those before they can reach this one.
        Rectangle {
            anchors.fill: parent
            color: Qt.rgba(0, 0, 0, 0.7)
            MouseArea {
                anchors.fill: parent
                onClicked: root._close()
            }
        }

        // Faked drop shadow (Tauri: 0 24px 48px rgba(0,0,0,0.4)) — see header.
        Rectangle {
            x: card.x
            y: card.y + 12
            width: card.width
            height: card.height
            radius: theme.radiusLg
            color: Qt.rgba(0, 0, 0, 0.4)
            opacity: 0.5
        }

        Rectangle {
            id: card
            width: Math.min(root.width * 0.9, 420)
            height: Math.min(root.height * 0.85, bodyCol.implicitHeight)
            x: Math.round((root.width - width) / 2)
            y: Math.round((root.height - height) / 2)
            radius: theme.radiusLg
            color: theme.surfaceCard
            clip: true

            // FIRST child: swallows clicks so they never reach the backdrop.
            MouseArea { anchors.fill: parent }

            // `.modal { overflow-y: auto; max-height: 85vh }` — the WHOLE card
            // scrolls, header included; the header is not sticky here.
            Flickable {
                id: flick
                anchors.fill: parent
                contentWidth: width
                contentHeight: bodyCol.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                clip: true

                Column {
                    id: bodyCol
                    width: flick.width
                    spacing: 0

                    // ---- Header (:76-89) ----------------------------------
                    // padding 24 / 24 / 16, align-items FLEX-START (the close
                    // button sits at the top of the row, not centred on it).
                    Item {
                        id: headerBox
                        width: parent.width
                        height: headerRow.height + 40

                        Row {
                            id: headerRow
                            x: 24
                            y: 24
                            width: parent.width - 48
                            spacing: 16

                            // .musician-icon — 48px circle, User glyph 24.
                            Rectangle {
                                width: 48
                                height: 48
                                radius: 24
                                color: theme.surfaceElevated
                                QbzIcon {
                                    anchors.centerIn: parent
                                    name: "user"
                                    width: 24
                                    height: 24
                                    tintName: "muted"
                                }
                            }

                            // .musician-details — column, gap 4.
                            Column {
                                width: parent.width - 48 - 16 - 32 - 16
                                spacing: 4

                                Text {
                                    width: parent.width
                                    text: root.doc.name || ""
                                    color: theme.textPrimary
                                    font.pixelSize: 20
                                    font.weight: theme.weightSemibold
                                    elide: Text.ElideRight
                                }
                                Text {
                                    width: parent.width
                                    text: root._capitalize(root.doc.role || "")
                                    color: theme.textMuted
                                    font.pixelSize: 14
                                    elide: Text.ElideRight
                                }
                            }

                            // .close-btn — 32x32, radius 8, surface-elevated;
                            // muted glyph that goes text-primary on hover, and
                            // the fill goes bg-hover. (The page's back button
                            // was 36px and had no colour change — this one is
                            // its own control.)
                            Item {
                                id: closeCell
                                width: 32
                                height: 32
                                activeFocusOnTab: true
                                Keys.onPressed: function (event) {
                                    if (event.key === Qt.Key_Return
                                        || event.key === Qt.Key_Enter
                                        || event.key === Qt.Key_Space) {
                                        root._close()
                                        event.accepted = true
                                    }
                                }

                                Rectangle {
                                    anchors.fill: parent
                                    radius: theme.radiusSm
                                    color: closeArea.containsMouse
                                        ? theme.bgHover : theme.surfaceElevated
                                    QbzIcon {
                                        anchors.centerIn: parent
                                        name: "x"
                                        width: 18
                                        height: 18
                                        tintName: closeArea.containsMouse
                                            ? "textPrimary" : "muted"
                                    }
                                    MouseArea {
                                        id: closeArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: root._close()
                                    }
                                }

                                FocusRing {
                                    host: closeCell
                                    radius: theme.radiusSm + 3
                                }
                            }
                        }

                        Rectangle {
                            anchors.bottom: parent.bottom
                            width: parent.width
                            height: 1
                            color: theme.borderSubtle
                        }
                    }

                    // ---- Content (.modal-content, padding 24) -------------
                    Column {
                        width: parent.width
                        leftPadding: 24
                        rightPadding: 24
                        topPadding: 24
                        bottomPadding: 24
                        spacing: 0

                        readonly property real contentWidth: width - 48

                        // ==== Block 1 — "Known for" (:94-106) =============
                        // DEAD in Tauri (`bands` is hardcoded empty on both
                        // resolver return paths, in BOTH backends) and made
                        // real by DV-2. Absent, not empty, when there is
                        // nothing to show.
                        Column {
                            visible: root.bands.length > 0
                            width: parent.contentWidth
                            spacing: 12

                            Text {
                                text: root.t("Known for")
                                color: theme.textMuted
                                font.pixelSize: 11
                                font.weight: theme.weightSemibold
                                font.capitalization: Font.AllUppercase
                                font.letterSpacing: 0.5
                            }

                            // .bands-list — a VERTICAL list, gap 8. Not
                            // clickable: Tauri renders plain rows here (the
                            // horizontal chips on the PAGE are the clickable
                            // ones), so there is nothing to make live.
                            Column {
                                width: parent.width
                                spacing: 8

                                Repeater {
                                    model: root.bands
                                    delegate: Row {
                                        required property var modelData
                                        width: parent ? parent.width : 0
                                        spacing: 10

                                        QbzIcon {
                                            anchors.verticalCenter: parent.verticalCenter
                                            name: "music"
                                            width: 14
                                            height: 14
                                            tintName: "muted"
                                        }
                                        Text {
                                            anchors.verticalCenter: parent.verticalCenter
                                            text: modelData.name || ""
                                            color: theme.textPrimary
                                            font.pixelSize: 14
                                        }
                                    }
                                }
                            }
                        }

                        // ==== Block 2 — the explanatory box (:109-114) ====
                        // Always rendered. Its own 24px top margin, which does
                        // NOT collapse into the container's padding — see the
                        // LIVE GEOMETRY note in the header.
                        Column {
                            width: parent.contentWidth
                            topPadding: 24

                            Rectangle {
                                width: parent.width
                                height: infoRow.height + 32
                                radius: theme.radiusMd
                                color: theme.surfaceElevated

                                Row {
                                    id: infoRow
                                    x: 16
                                    y: 16
                                    width: parent.width - 32
                                    spacing: 12

                                    // The glyph is nudged 2px down so it sits
                                    // on the first line of copy, not above it.
                                    QbzIcon {
                                        y: 2
                                        name: "info"
                                        width: 16
                                        height: 16
                                        tintName: "muted"
                                    }
                                    Text {
                                        width: parent.width - 16 - 12
                                        text: root.explanatoryCopy
                                        color: theme.textSecondary
                                        font.pixelSize: 13
                                        lineHeight: 1.5
                                        lineHeightMode: Text.ProportionalHeight
                                        wrapMode: Text.WordWrap
                                    }
                                }
                            }
                        }

                        // ==== Block 3 — the CTA (:117-123) ================
                        // Present and unreachable — see showArtistCta.
                        Column {
                            visible: root.showArtistCta
                            width: parent.contentWidth
                            topPadding: 24
                            spacing: 20

                            Rectangle {
                                width: parent.width
                                height: 1
                                color: theme.borderSubtle
                            }

                            Item {
                                id: ctaCell
                                width: parent.width
                                height: 44
                                activeFocusOnTab: true
                                Keys.onPressed: function (event) {
                                    if (event.key === Qt.Key_Return
                                        || event.key === Qt.Key_Enter
                                        || event.key === Qt.Key_Space) {
                                        root._openArtist()
                                        event.accepted = true
                                    }
                                }

                                // width 100%, padding 12/20, radius 10 — a
                                // literal, not a token: 10 sits between
                                // Radius.sm (8) and Radius.md (12), and this is
                                // not a pill at 44px tall, so ADR-008 has
                                // nothing to say about it.
                                Rectangle {
                                    anchors.fill: parent
                                    radius: 10
                                    color: ctaArea.containsMouse
                                        ? theme.accentHover : theme.accent

                                    Text {
                                        anchors.centerIn: parent
                                        text: root.t("View Artist Page")
                                        // The on-accent glyph selector, this
                                        // port's owner-approved WCAG deviation
                                        // (QbzTheme.qml:337) — Tauri hardcodes
                                        // --btn-primary-text #ffffff, which is
                                        // unreadable on the light accents some
                                        // of the 30 themes carry.
                                        color: theme.accentGlyphColor
                                        font.pixelSize: 14
                                        font.weight: theme.weightSemibold
                                    }

                                    MouseArea {
                                        id: ctaArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: root._openArtist()
                                    }
                                }

                                FocusRing {
                                    host: ctaCell
                                    radius: 13
                                }
                            }
                        }
                    }
                }
            }

            QbzScrollBar {
                target: flick
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.top: parent.top
                anchors.bottom: parent.bottom
            }
        }
    }
}
