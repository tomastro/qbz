// Search "cortinilla" — the live as-you-type dropdown under the header
// search box (search/Cortinilla.slint, phase 15). A plain Item overlay
// mounted as a LAST child of AppShell (NOT a Popup — a Popup grabs the
// pointer and would kill typing in the header field, the exact reason the
// Slint uses a plain Rectangle too).
//
// Payload: QbzSearch.cortinillaJson (search_qt.rs CortinillaData: query /
// top / sections with controller flat indices). Keyboard selection rides
// selectedIndex + cortinillaScrollY (content-space top-y of the selected
// row — scrolled into view here). Row activation bubbles to the
// QbzSearch.cortinilla* invokables; the field clear goes through
// root.header.clearSearch().

import QtQuick
import QtQuick.Controls
import QtQuick.Effects
import com.blitzfc.qbz
import "../controls"
import "../theme"

Item {
    id: root
    anchors.fill: parent
    // Open AND the live query long enough to have meaningful results — the
    // same >= 2 gate the controller applies, so a 1-char query never flashes
    // a half-empty panel (Cortinilla.slint:190-191). cortinillaQuery is
    // published synchronously per keystroke, so this tracks what the user
    // has typed rather than what last finished loading.
    visible: QbzSearch.cortinillaOpen && QbzSearch.cortinillaQuery.length >= 2

    // The host HeaderBar (for clearSearch on row activation).
    property var headerBar: null

    QbzTheme { id: theme }

    // The document is applied IMPERATIVELY, not bound, so the scroll offset
    // survives a republish. `cortinillaJson` is republished wholesale when
    // the artwork pass lands (~1.5s after the rows), and a bound Repeater
    // model would be a full model reset: every delegate destroyed and
    // rebuilt, contentHeight momentarily 0, and StopAtBounds clamping
    // contentY to 0 — the user is yanked back to the top mid-read. This is
    // the port's own documented cure (MyQbzGridView.applyItems).
    property var doc: ({})
    property var sections: []
    readonly property bool hasTop: doc.top !== undefined && doc.top !== null
    readonly property bool empty: !hasTop && sections.length === 0 && !QbzSearch.cortinillaLoading

    function parseDoc() {
        try {
            return JSON.parse(QbzSearch.cortinillaJson)
        } catch (e) {
            return ({})
        }
    }

    property real _restoreY: 0
    function applyDoc() {
        root._restoreY = bodyFlick.contentY
        root.doc = parseDoc()
        root.sections = root.doc.sections || []
        // NOT forceLayout(): that is a QQuickItemView method (ListView /
        // GridView), and this is a plain Flickable + Column. The Column
        // re-layouts on the polish pass, so contentHeight is still the OLD
        // value here and clamping against it would be wrong. Restore once
        // layout has settled instead; Qt.callLater coalesces, so a burst of
        // republishes costs one restore.
        Qt.callLater(root._restoreScroll)
    }
    function _restoreScroll() {
        var maxY = Math.max(0, bodyFlick.contentHeight - bodyFlick.height)
        bodyFlick.contentY = Math.max(0, Math.min(root._restoreY, maxY))
    }

    function activateRow(row) {
        var fi = row.flatIndex
        if (fi === undefined || fi === null) {
            console.warn("cortinilla: row has no flatIndex, action dropped —",
                         JSON.stringify(row))
            return
        }
        // Dispatch first: clearing the field can invalidate the snapshot the
        // controller resolves this index against.
        QbzSearch.cortinillaRowClicked(fi)
        if (root.headerBar) root.headerBar.clearSearch()
    }

    function menuEntries(row) {
        var t = QbzSession.tr
        var r = QbzSession.trRev
        if (row.kind === "album") return [
            { "label": t("Open album", r), "icon": "library-big", "action": "open" },
            { "label": t("Play", r), "icon": "play-fill", "action": "play" },
            { "label": t("Play next", r), "icon": "list-start", "action": "next" },
            { "label": t("Play later", r), "icon": "list-plus", "action": "later" },
            { "label": t("Add to queue", r), "icon": "list-end", "action": "queue" }
        ]
        if (row.kind === "artist") {
            var artistRows = [
                { "label": t("Open artist", r), "icon": "user", "action": "open" }
            ]
            if (row.source !== "local")
                artistRows.push({ "label": t("Play", r), "icon": "play-fill", "action": "play" })
            return artistRows
        }
        if (row.kind === "track") {
            var trackRows = [
                { "label": t("Play", r), "icon": "play-fill", "action": "play" },
                { "label": t("Play next", r), "icon": "list-start", "action": "next" },
                { "label": t("Play later", r), "icon": "list-plus", "action": "later" },
                { "label": t("Add to queue", r), "icon": "list-end", "action": "queue" },
                { "label": t("Add to playlist", r), "icon": "list-music", "action": "add-to-playlist" }
            ]
            // QoL round: gated on the payload actually carrying the album
            // (a Qobuz id or a local group key — search_qt::CortRow.albumId).
            if ((row.albumId || "") !== "")
                trackRows.push({ "label": t("Open containing album", r), "icon": "disc-3", "action": "open-album" })
            return trackRows
        }
        if (row.kind === "playlist") return [
            { "label": t("Open playlist", r), "icon": "list-music", "action": "open" },
            { "label": t("Play", r), "icon": "play-fill", "action": "play" },
            { "label": t("Play next", r), "icon": "list-start", "action": "next" },
            { "label": t("Play later", r), "icon": "list-plus", "action": "later" },
            { "label": t("Add to queue", r), "icon": "list-end", "action": "queue" }
        ]
        return []
    }

    function menuAction(row, action) {
        var fi = row.flatIndex
        if (fi === undefined || fi === null)
            return
        QbzSearch.cortinillaMenuAction(fi, action)
        if ((action === "open" || action === "open-album" || action === "play"
                || action === "add-to-playlist") && root.headerBar)
            root.headerBar.clearSearch()
    }

    Component.onCompleted: root.applyDoc()

    // The centered header search box width (HeaderBar's responsive rule) —
    // the click-through gaps below leave it free.
    // Must track HeaderBar's own rule EXACTLY (HeaderBar.qml:642), including
    // the 60px the field gives up when the section nav sits in the header —
    // the scrims below leave this span free, and a span 60px too wide leaves
    // a 30px dead strip on each side of the field that neither dismisses the
    // panel nor reaches the input.
    readonly property int searchBoxWidth:
        (root.width < 960 ? 179 : 256) - (QbzShell.navInSidebar ? 0 : 60)
    readonly property int panelWidth: Math.min(440, Math.max(0, root.width - 24))
    // Cap so the panel NEVER covers the now-playing bar
    // (Cortinilla.slint:263-275). The bar term is NPB-MODE AWARE: mode 2 is
    // the small bar, every other mode is the large one. A hardcoded 42 here
    // let the panel run up to 70px into a 112px bar — latent while the
    // Qobuz-only panel held <= 14 rows, reachable the moment the local
    // sections make it scroll.
    readonly property int npbHeight:
        QbzShell.npbMode === 2 ? theme.npbSmallHeight : theme.npbLargeHeight
    readonly property int scrollCap:
        root.height - theme.headerHeight - 6 - root.npbHeight - 16 - 12

    // --- Focus-driven lifetime (QoL round, replaces the Slint idle timer) --
    // The 4.5s/30s idle countdown was inherited from Slint, which could not
    // keep the panel open while the field kept focus. The Qt rule is the one
    // users expect of a combobox: while the header searchInput holds
    // activeFocus the panel STAYS open; what closes it is an explicit act —
    // a click outside (the scrims below, which also break the field's
    // focus), Escape (HeaderBar's duck-walk to the shell root), activating a
    // row, or a page change. No timer, per the repaint-pulse doctrine.
    Connections {
        target: QbzSearch
        function onCortinillaJsonChanged() { root.applyDoc() }
        // Keyboard scroll-into-view: the controller publishes the selected
        // row's content-space top-y; nudge the Flickable so it is visible.
        function onCortinillaScrollYChanged() {
            var y = QbzSearch.cortinillaScrollY
            if (y < bodyFlick.contentY) {
                bodyFlick.contentY = Math.max(0, y)
            } else if (y + 68 > bodyFlick.contentY + bodyFlick.height) {
                bodyFlick.contentY = y + 68 - bodyFlick.height
            }
        }
    }

    Connections {
        target: QbzShell
        // A page change dismisses (AppShell's tracked-view close hook).
        function onCurrentViewChanged() { QbzSearch.cortinillaDismiss() }
    }

    // --- Click-outside scrims (any click outside the panel dismisses) ----
    // With the focus-driven lifetime, the scrim is what "breaks the focus":
    // it hands the keyboard to the shell root (the same duck-walk HeaderBar's
    // Escape does — merely clearing focus leaves AppShell's Keys handler
    // receiving nothing) and then dismisses.
    function dismissOutside() {
        var p = root
        while (p.parent) {
            if (p.parent.isQbzShellRoot === true) {
                p.parent.forceActiveFocus()
                break
            }
            p = p.parent
        }
        QbzSearch.cortinillaDismiss()
    }
    // 1) everything BELOW the header.
    MouseArea {
        visible: root.visible
        x: 0
        y: theme.headerHeight
        width: root.width
        height: root.height - theme.headerHeight
        onClicked: root.dismissOutside()
    }
    // 2) header strip LEFT of the search box.
    MouseArea {
        visible: root.visible
        x: 0
        y: 0
        width: (root.width - root.searchBoxWidth) / 2
        height: theme.headerHeight
        onClicked: root.dismissOutside()
    }
    // 3) header strip RIGHT of the search box.
    MouseArea {
        visible: root.visible
        x: (root.width + root.searchBoxWidth) / 2
        y: 0
        width: root.width - x
        height: theme.headerHeight
        onClicked: root.dismissOutside()
    }

    // --- The panel ----------------------------------------------------------
    // Drop shadow (Cortinilla.slint:314-316: blur 24, offset-y 6,
    // Theme.card-shadow). Same idiom as the immersive twin
    // (ImmersiveSearchCortinilla.qml): a mirror Rectangle behind the panel,
    // blurred by MultiEffect, skipped when there are no shaders. This port
    // runs on the GPU (OpenGL RHI, measured), but the offscreen smoke forces
    // software, so the guard is real rather than defensive.
    //
    // NOTE: NavFlyout.qml still has no shadow and says so; adding one there
    // is its own parity pass, not this contract's. The two dropdowns will
    // look slightly different until that lands.
    readonly property bool noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    Rectangle {
        visible: root.visible && !root.noShaders
        x: panel.x
        y: panel.y + 6
        width: panel.width
        height: panel.height
        radius: panel.radius
        color: theme.cardShadow
        layer.enabled: true
        layer.effect: MultiEffect {
            blurEnabled: true
            blurMax: 48
            blur: 0.5 // 24/48
        }
    }
    Rectangle {
        id: panel
        x: (root.width - root.panelWidth) / 2
        y: theme.headerHeight + 6
        width: root.panelWidth
        height: Math.min(bodyColumn.height + 12, root.scrollCap + 12)
        radius: theme.radiusSm
        color: theme.surfaceMain
        border.width: 1
        border.color: theme.borderMuted
        clip: true

        // Swallow clicks that land on the panel background so they do not
        // fall through to the dismiss scrims underneath.
        MouseArea {
            anchors.fill: parent
            acceptedButtons: Qt.AllButtons
            onClicked: {}
        }

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

                // --- Loading skeleton --------------------------------
                // Shown on a cache MISS. There IS a cached instant paint now
                // (rulings R1/R6, default on, ui_prefs opt-out
                // `cortinilla_instant_paint`): on a hit the controller sends
                // the cached rows and loading=false together, so this block
                // never appears. It stays fully implemented as the opt-out
                // target and the cold path — it is not dead code.
                Column {
                    id: skeleton
                    visible: QbzSearch.cortinillaLoading
                    width: parent.width
                    topPadding: 4
                    property bool pulse: false
                    // ONE host Timer flipping ONE bool for all five rows —
                    // never a per-delegate animation. Under reduce-motion it
                    // drops to a ~8fps coarse tick, which is what the
                    // reference switches its pulse's time source to
                    // (Cortinilla.slint:128-131). That file records why: this
                    // pulse was the main cost of opening the cortinilla on
                    // weak GPUs, because every frame was a full-window
                    // repaint.
                    Timer {
                        interval: QbzShell.reduceMotion ? 2000 : 700
                        running: skeleton.visible
                        repeat: true
                        onTriggered: skeleton.pulse = !skeleton.pulse
                    }
                    Repeater {
                        model: 5
                        delegate: Item {
                            width: bodyColumn.width
                            height: 68
                            Rectangle {
                                x: 10
                                anchors.verticalCenter: parent.verticalCenter
                                width: 48
                                height: 48
                                radius: 4
                                color: theme.surfaceElevated
                                opacity: skeleton.pulse ? 0.48 : 0.16
                                Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                            }
                            Column {
                                x: 70
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 5
                                Rectangle {
                                    width: 150
                                    height: 11
                                    radius: 3
                                    color: theme.surfaceElevated
                                    opacity: skeleton.pulse ? 0.48 : 0.16
                                    Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                                }
                                Rectangle {
                                    width: 96
                                    height: 9
                                    radius: 3
                                    color: theme.surfaceElevated
                                    opacity: skeleton.pulse ? 0.48 : 0.16
                                    Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                                }
                                Rectangle {
                                    width: 72
                                    height: 8
                                    radius: 3
                                    color: theme.surfaceElevated
                                    opacity: skeleton.pulse ? 0.48 : 0.16
                                    Behavior on opacity { NumberAnimation { duration: 650; easing.type: Easing.InOutQuad } }
                                }
                            }
                        }
                    }
                }

                // --- Loaded content ----------------------------------------
                Column {
                    visible: !QbzSearch.cortinillaLoading
                    width: parent.width

                    // No results.
                    Item {
                        visible: root.empty
                        width: parent.width
                        height: 28
                        Text {
                            x: 14
                            anchors.verticalCenter: parent.verticalCenter
                            // The LIVE query, matching the reference
                            // (Cortinilla.slint:373 reads
                            // SearchState.cortinilla-query, not the payload's
                            // echo) — otherwise the empty state quotes the
                            // previous query while the user reads it.
                            text: QbzSession.tr("No results for", QbzSession.trRev) + " “" + QbzSearch.cortinillaQuery + "”"
                            color: theme.textMuted
                            font.pixelSize: 12
                        }
                    }

                    // Top result (flat-index 0).
                    Column {
                        visible: root.hasTop
                        width: parent.width
                        leftPadding: 6
                        rightPadding: 6
                        topPadding: 4
                        Text {
                            x: 10
                            height: 22
                            text: QbzSession.tr("Top result", QbzSession.trRev)
                            color: theme.textMuted
                            font.pixelSize: 11
                            font.weight: theme.weightSemibold
                            verticalAlignment: Text.AlignVCenter
                        }
                        CortRow { row: root.hasTop ? root.doc.top : ({}); blockPadding: 6 }
                    }

                    // Sections.
                    Repeater {
                        model: root.sections
                        delegate: Column {
                            id: sectionCol
                            required property var modelData
                            // Named alias: the inner Repeater below also has
                            // a `modelData`, and which one an unqualified
                            // reference resolves to depends on whether the
                            // inner delegate declares required properties.
                            // It happens to work today; one added `required`
                            // would silently flip every row to render its
                            // SECTION instead. Both are named explicitly now.
                            readonly property var section: modelData
                            width: bodyColumn.width
                            leftPadding: 6
                            rightPadding: 6
                            topPadding: 4

                            Item {
                                width: parent.width
                                height: 24
                                Text {
                                    anchors.left: parent.left
                                    anchors.leftMargin: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: modelData.title
                                    color: theme.textMuted
                                    font.pixelSize: 11
                                    font.weight: theme.weightSemibold
                                }
                                Text {
                                    visible: modelData.hasMore === true
                                    anchors.right: parent.right
                                    anchors.rightMargin: 10
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: QbzSession.tr("View more", QbzSession.trRev)
                                    color: vmArea.containsMouse ? theme.textPrimary : theme.accent
                                    font.pixelSize: 11
                                    font.weight: theme.weightMedium
                                    MouseArea {
                                        id: vmArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: {
                                            if (root.headerBar) root.headerBar.clearSearch()
                                            QbzSearch.cortinillaViewMore(modelData.kind)
                                        }
                                    }
                                }
                            }
                            Repeater {
                                model: sectionCol.section.rows
                                delegate: CortRow {
                                    required property var modelData
                                    row: modelData
                                    blockPadding: 6
                                }
                            }
                        }
                    }
                }
            }
        }

        QbzScrollBar {
            target: bodyFlick
            // 10px gutter, not the primitive's default 14 — the reference
            // overrides it the same way (Cortinilla.slint:471).
            width: 10
            anchors.right: parent.right
            anchors.top: parent.top
            anchors.bottom: parent.bottom
        }
    }

    // --- One result row (CortinillaResultRow) ------------------------------
    component CortRow: Rectangle {
        id: cortRow
        property var row: ({})
        // A QML Column's padding does NOT size its children, so a plain
        // `parent.width` made every row 12px wider than its block: clip hid
        // the overflow, but the highlight ran flush to the right edge while
        // staying inset 6px on the left — asymmetric.
        //
        // The padding is passed IN rather than read off `parent`. Reading it
        // through the parent chain works at runtime but is invisible to
        // qmllint (`parent` is typed QQuickItem, which has no leftPadding),
        // so a wrong mount would fail silently at runtime instead of loudly
        // in the audit — and parent-chain bindings are a documented trap in
        // this port.
        property int blockPadding: 0
        width: parent ? parent.width - 2 * blockPadding : 0
        height: 68
        radius: theme.radiusSm
        readonly property bool active: QbzSearch.cortinillaSelectedIndex === (row.flatIndex ?? -2)
        color: (active || rowArea.containsMouse) ? theme.surfaceHover : "transparent"

        // Accent bar on the keyboard-active row (distinct from plain hover).
        Rectangle {
            visible: parent.active
            x: 0
            y: 4
            width: 3
            height: parent.height - 8
            radius: 2
            color: theme.accent
        }
        Rectangle {
            x: 10
            anchors.verticalCenter: parent.verticalCenter
            width: 48
            height: 48
            // Artists read better as a circle; everything else is a tile.
            radius: row.kind === "artist" ? 24 : 5
            color: theme.surfaceElevated
            clip: true
            RoundedImage {
                anchors.fill: parent
                source: row.artPath || ""
                radius: row.kind === "artist" ? 24 : 5
            }
        }
        Column {
            anchors.left: parent.left
            anchors.leftMargin: 70
            anchors.right: menuButton.left
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            spacing: 1
            Text {
                width: parent.width
                text: row.title || ""
                color: theme.textPrimary
                font.pixelSize: 14
                font.weight: theme.weightMedium
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: row.subtitle || ""
                color: theme.textMuted
                font.pixelSize: 12
                elide: Text.ElideRight
            }
            Text {
                visible: (row.qualityDetail || "") !== ""
                width: parent.width
                text: row.qualityDetail || ""
                color: theme.textSecondary
                font.pixelSize: 10
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }
        }
        MouseArea {
            id: rowArea
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            anchors.right: menuButton.left
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: function (mouse) {
                if (mouse.button === Qt.RightButton) {
                    rowMenuLoader.active = true
                    rowMenuLoader.item.openAtCursor(rowArea, mouse.x, mouse.y)
                    return
                }
                root.activateRow(row)
            }
        }
        QbzIconButton {
            id: menuButton
            anchors.right: parent.right
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            btnSize: 28
            iconSize: 14
            name: "ellipsis"
            onClicked: {
                rowMenuLoader.active = true
                rowMenuLoader.item.openBelowRight(menuButton)
            }
            HoverHandler { id: menuHover }
            ToolTip.visible: menuHover.hovered
            ToolTip.text: QbzSession.tr("More options", QbzSession.trRev)
            ToolTip.delay: 350
        }
        Loader {
            id: rowMenuLoader
            active: false
            sourceComponent: CardMenu {
                menuWidth: 196
                entries: root.menuEntries(cortRow.row)
                onPicked: function (action) {
                    root.menuAction(cortRow.row, action)
                }
            }
        }
    }
}
