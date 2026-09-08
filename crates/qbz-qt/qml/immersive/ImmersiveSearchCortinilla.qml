// ImmersiveSearchCortinilla — the live as-you-type dropdown under the
// immersive header search field (ImmersiveSearchCortinilla.slint:33-366;
// contract 2026-08-02-immersive-port §12 B5). Mounted by ImmersiveView as
// layer 7 (TOPMOST), anchors.fill.
//
// Like the desktop twin (shell/Cortinilla.qml) this is a plain Item, NOT a
// Popup — a Popup grabs the pointer and kills typing in the search field.
// Differences from the desktop twin (D8): GLASS immersive palette (dark
// translucent panel, white-alpha text) instead of the app theme tokens;
// sections ONLY (Artists / Albums / Playlists — NO top-result hero, NO
// "View more", NO track rows); reads QbzImmersive.immSearch* (never
// QbzSearch.cortinilla*); NO idle auto-close (Slint has no such timer) and
// no view-change dismiss — the immersive host never navigates.
//
// Data: QbzImmersive.immSearchJson (search_qt.rs CortinillaData: query /
// top:null / sections with controller flat indices from 1). Keyboard
// selection rides immSearchSelectedIndex + immSearchScrollY (content-space
// top-y of the selected row, 56/24/4 layout — scrolled into view here).
// Row activation -> QbzImmersive.searchRowActivated(flatIndex); the Rust
// controller closes the dropdown, clears the field and ACTS ON PLAYBACK —
// the user STAYS in immersive.
//
// All colors are Slint RRGGBBAA converted to Qt #AARRGGBB.

import QtQuick
import QtQuick.Effects
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    // Keyboard accent (the host feeds QbzShell.ambientAccent so the active
    // row matches the atmosphere; the Slint passes it in from the host the
    // same way, default #3fd9c8).
    property color accent: "#3fd9c8"

    // Only render (and only consume events) when the search is open AND the
    // live query is long enough to have meaningful results — mirrors the
    // controller's >= 2 gate so a 1-char query never flashes a half-empty
    // panel (ImmersiveSearchCortinilla.slint:165-167). searchInputText is
    // the Qt mirror of Slint's search-query.
    visible: QbzImmersive.immSearchOpen
        && QbzImmersive.searchInputText.trim().length >= 2

    // --- Geometry (all dims passed/computed here — no parent-binding
    // loops) ---------------------------------------------------------------
    readonly property int panelWidth: 380
    // The TOP of the panel (header-band y + field height + a small gap).
    readonly property int anchorY: 58
    // The immersive player bar's reserved bottom band (bar height + bottom
    // inset + a breathing gap) — the panel's bottom edge stays ABOVE it.
    readonly property int bottomReserved: 138
    // scroll-cap = window-height - anchor-y - bottom-reserved - 12 (the
    // 12 = the panel box's top+bottom padding).
    readonly property int scrollCap: root.height - anchorY - bottomReserved - 12

    // The centered search field's span (same width math as the header
    // field): the header band across that span stays un-scrimmed so the
    // field keeps typing/focus.
    readonly property int fieldWidth: Math.min(root.width - 48, 420)
    readonly property int fieldLeft: (root.width - root.fieldWidth) / 2
    // The header band's vertical extent (field 36px + the 6px gap).
    readonly property int bandBottom: anchorY - 6

    readonly property var doc: parseDoc()
    function parseDoc() {
        try {
            return JSON.parse(QbzImmersive.immSearchJson)
        } catch (e) {
            return ({})
        }
    }
    readonly property var sections: doc.sections || []

    // Keyboard scroll-into-view (the desktop Cortinilla.qml:63-71 idiom):
    // the controller publishes the selected row's content-space top-y;
    // nudge the Flickable so that row is visible. 56px is the row height.
    Connections {
        target: QbzImmersive
        function onImmSearchScrollYChanged() {
            var y = QbzImmersive.immSearchScrollY
            if (y < bodyFlick.contentY) {
                bodyFlick.contentY = Math.max(0, y)
            } else if (y + 56 > bodyFlick.contentY + bodyFlick.height) {
                bodyFlick.contentY = y + 56 - bodyFlick.height
            }
        }
    }

    // --- Click-outside scrims (ANY click outside the panel dismisses) ----
    // Transparent MouseAreas tile the whole window EXCEPT the panel
    // (declared later -> on top, keeps its own clicks) and the header band
    // across the search field's horizontal span.
    //   1) everything BELOW the header band (panels, player bar region)
    MouseArea {
        visible: root.visible
        x: 0
        y: root.bandBottom
        width: root.width
        height: root.height - y
        onClicked: QbzImmersive.dismissSearch()
    }
    //   2) header band, LEFT of the search field
    MouseArea {
        visible: root.visible
        x: 0
        y: 0
        width: root.fieldLeft
        height: root.bandBottom
        onClicked: QbzImmersive.dismissSearch()
    }
    //   3) header band, RIGHT of the search field
    MouseArea {
        visible: root.visible
        x: root.fieldLeft + root.fieldWidth
        y: 0
        width: root.width - x
        height: root.bandBottom
        onClicked: QbzImmersive.dismissSearch()
    }

    // --- The panel ---------------------------------------------------------
    // Anchored under the centered search field; its total height is capped
    // so the bottom edge stays ABOVE the player bar. A tall result set
    // scrolls inside the Flickable; a short one collapses.
    // Shadow: 32 / #00000066 / offset-y 8 (the ImmersiveView shadow idiom).
    Rectangle {
        visible: root.visible
        x: (root.width - root.panelWidth) / 2
        y: root.anchorY + 8
        width: root.panelWidth
        height: Math.min(bodyColumn.height + 12, root.scrollCap + 12)
        radius: 16
        color: "#66000000"
        layer.enabled: !root._noShaders
        layer.effect: MultiEffect {
            blurEnabled: true
            blurMax: 64
            blur: 0.5 // 32/64
        }
    }
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null

    Rectangle {
        id: panel
        visible: root.visible
        x: (root.width - root.panelWidth) / 2
        y: root.anchorY
        width: root.panelWidth
        height: Math.min(bodyColumn.height + 12, root.scrollCap + 12)
        radius: 16
        color: "#94000000"
        border.width: 1
        border.color: "#1affffff"
        clip: true

        Flickable {
            id: bodyFlick
            anchors.fill: parent
            anchors.topMargin: 6
            anchors.bottomMargin: 6
            contentWidth: width
            contentHeight: bodyColumn.height
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: bodyColumn
                width: bodyFlick.width

                // --- Loading skeleton (4 rows, glass-white boxes like
                // ImmersiveSkeletonRow). Skeleton-only HERE: the instant
                // cached paint landed on the DESKTOP cortinilla only. The
                // immersive dropdown is a different surface with a much
                // smaller payload, so it was left out deliberately rather
                // than forgotten. ------------------------------------------
                Column {
                    id: immSkeleton
                    visible: QbzImmersive.immSearchLoading
                    width: parent.width
                    topPadding: 4
                    property bool pulse: false
                    // Referenced BY ID, not through `parent`. A Timer derives
                    // from QtObject, which has no `parent` property, so an
                    // unqualified `parent` here resolved up the scope chain
                    // and out of this block entirely — the timer ran forever
                    // and `pulse` never changed, so the skeleton never
                    // pulsed. Same fix as the desktop twin.
                    Timer {
                        interval: QbzShell.reduceMotion ? 2000 : 700
                        running: immSkeleton.visible
                        repeat: true
                        onTriggered: immSkeleton.pulse = !immSkeleton.pulse
                    }
                    Repeater {
                        model: 4
                        delegate: Item {
                            width: bodyColumn.width
                            height: 56
                            Rectangle {
                                x: 10
                                anchors.verticalCenter: parent.verticalCenter
                                width: 40
                                height: 40
                                radius: 4
                                color: "#ffffff"
                                opacity: immSkeleton.pulse ? 0.32 : 0.08
                                Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                            }
                            Column {
                                x: 62
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 8
                                Rectangle {
                                    width: 150
                                    height: 11
                                    radius: 3
                                    color: "#ffffff"
                                    opacity: immSkeleton.pulse ? 0.32 : 0.08
                                    Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                                }
                                Rectangle {
                                    width: 96
                                    height: 9
                                    radius: 3
                                    color: "#ffffff"
                                    opacity: immSkeleton.pulse ? 0.32 : 0.08
                                    Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                                }
                            }
                        }
                    }
                }

                // --- Loaded content ----------------------------------------
                Column {
                    visible: !QbzImmersive.immSearchLoading
                    width: parent.width

                    // No results: explicit empty-state line.
                    Item {
                        visible: root.sections.length === 0
                        width: parent.width
                        height: 28
                        Text {
                            x: 14
                            anchors.verticalCenter: parent.verticalCenter
                            text: QbzSession.tr("No results for", QbzSession.trRev)
                                + " “" + (root.doc.query || "") + "”"
                            color: "#80ffffff"
                            font.pixelSize: 12
                        }
                    }

                    // Result sections (Artists / Albums / Playlists, then the
                    // local-album section). NO "View more" — immersive has
                    // no results page to navigate to.
                    Repeater {
                        model: root.sections
                        delegate: Column {
                            required property var modelData
                            width: bodyColumn.width
                            leftPadding: 6
                            rightPadding: 6
                            topPadding: 4

                            // Section header — title only, 24px, 11px
                            // semibold #ffffff80.
                            Item {
                                width: parent.width
                                height: 24
                                Text {
                                    anchors.left: parent.left
                                    anchors.leftMargin: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: modelData.title
                                    color: "#80ffffff"
                                    font.pixelSize: 11
                                    font.weight: 600
                                }
                            }
                            Repeater {
                                model: modelData.rows
                                delegate: ImmRow { row: modelData }
                            }
                        }
                    }
                }
            }
        }

        QbzScrollBar {
            target: bodyFlick
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }

    // --- One result row (ImmersiveResultRow :30-102) -----------------------
    // 56px, radius 8; keyboard-active #ffffff26 + the 3px accent bar, plain
    // hover #ffffff14. Hover is PURELY VISUAL — it deliberately does NOT
    // write the selection, so a stray hover can never hijack the keyboard
    // selection that Enter activates (same lesson as the main cortinilla).
    component ImmRow: Rectangle {
        property var row: ({})
        width: parent ? parent.width : 0
        height: 56
        radius: 8
        readonly property bool active: QbzImmersive.immSearchSelectedIndex === (row.flatIndex ?? -2)
        color: active ? "#26ffffff"
            : (rowArea.containsMouse ? "#14ffffff" : "transparent")

        // Accent bar on the keyboard-active row (distinct from plain hover).
        Rectangle {
            visible: parent.active
            x: 0
            y: 4
            width: 3
            height: parent.height - 8
            radius: 2
            color: root.accent
        }
        // 40x40 thumb — circle for artists, tile otherwise.
        Rectangle {
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            width: 40
            height: 40
            radius: row.kind === "artist" ? 20 : 4
            color: "#14ffffff"
            clip: true
            RoundedImage {
                anchors.fill: parent
                source: row.artPath || ""
                radius: row.kind === "artist" ? 20 : 4
            }
        }
        Column {
            x: 62
            width: parent.width - 62 - 10
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2
            Text {
                width: parent.width
                text: row.title || ""
                color: "#ccffffff"
                font.pixelSize: 14
                font.weight: 500
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: row.subtitle || ""
                color: "#80ffffff"
                font.pixelSize: 12
                elide: Text.ElideRight
            }
        }
        MouseArea {
            id: rowArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: QbzImmersive.searchRowActivated(row.flatIndex)
        }
    }
}
