// Musician page — QML port of Tauri's MusicianPageView.svelte (469 lines, read
// in full for this port). The surface a CONTEXTUAL musician click lands on: a
// name, a role, and a grid of Qobuz albums whose title/artist matched the name.
//
// THE REFERENCE IS TAURI, NOT SLINT, for this module only (00-CONTRACT §0, the
// owner's ruling). Slint's musician page is cited nowhere below; where the two
// disagree Tauri wins, and the Slint page's own defects (no error state, so a
// network failure renders "No album appearances found."; a downsized 18px name)
// are not reproduced.
//
// Layout, read off the .svelte (every number is that file's):
//   :216-226  sticky header — padding 24/24/16 over a 56px row, 1px bottom
//             border, background bg-primary
//   :253-263  56x56 circular avatar, surface-elevated, User glyph 32, muted.
//             There is NO photo, ever: ResolvedMusician has no image field
//             (02-tauri-musician.md §1.3)
//   :271-283  name h1 24/700, role 14 muted CAPITALIZED
//   :285-311  content padding 24; section margin-bottom 32; section title
//             18/600 with a 14/400 muted "(N)" count
//   :314-344  Bands & Projects — chips, flex-wrap, gap 10, padding 10/16
//   :347-351  Appears On grid: repeat(auto-fill, minmax(160px, 1fr)), gap 20 —
//             a STRETCHING grid, so the cell width is computed, not fixed
//   :353-426  card: artwork 1:1 radius 8 (no shadow), title 14/500, artist
//             12 muted, meta row gap 8 margin-top 4 = date + role badge
//   :429-440  the three state boxes: column, centred, gap 12, padding 48,
//             muted 14
//   :443-468  Load more: 10/24 padded button, surface-elevated, radius 8,
//             disabled at 0.6
//
// WHAT THIS PAGE DELIBERATELY DIVERGES ON (00-CONTRACT §6, do not "fix" back):
//  - DV-15  NO per-view back button. Tauri draws a 36px .back-btn at the head
//           of its sticky header; in this port back/forward live permanently in
//           the shell HeaderBar, mounted above the router. The header stack
//           therefore starts at x=40 — Tauri's 24px padding plus the 16px gap
//           that sat between the button and the avatar, with the 36px button
//           itself removed. Same arithmetic ArtistReleasesView.qml:27-31 uses
//           for its x=48 (32 + 16).
//  - DV-19  the role badge (.role-badge, Tauri radius 10) drops to Radius.sm.
//           Size, padding, type and colour are Tauri's to the pixel; only the
//           corner changes. ADR-008 is Active and scoped to "any future UI
//           surface", and a 10px label in a 2/8 box at radius 10 is that ADR's
//           literal definition of a pill (owner ruling R5).
//  - §5.4   `releaseDate` renders IN FULL — "1994-09-27", not "1994". Tauri's
//           field is named `year` and its CSS class is `.year`, but the backend
//           assigns `release_date_original` verbatim and the page prints it
//           raw. Both backends do this deliberately. Truncating it would be
//           inference from a field NAME, which this contract forbids.
//  - §5.2   the page is FULLY KEYBOARD-OPERABLE. The Tauri spec's "no keyboard
//           handling, no focus management" is true of its BINDINGS and false of
//           its BEHAVIOUR: every control there is a native <button> and gets tab
//           order, Enter/Space and a focus ring from the browser for free. QML
//           grants none of it, so bare MouseAreas here would be a regression
//           against the reference, not parity. Every control below is an
//           activeFocusOnTab Item with an Enter/Space handler and a focus ring.
//           Non-activation keys are left unaccepted so they keep bubbling up to
//           the AppShell dispatcher (AppShell.qml:132-139) and the global
//           hotkeys still fire under this page.
//
// INHERITED TAURI BEHAVIOUR, kept on purpose (02-tauri-musician.md §1.4, §3.2):
//  - `roleOnAlbum` is the SAME string on every card — the backend echoes the
//    role the user clicked with (`role.clone()` on every row). It is honest:
//    per-album credit data does not exist.
//  - `total` is the raw Qobuz album-SEARCH total, which routinely exceeds what
//    pagination returns, so Load more can stay armed after the last real page.
//    That is the reference's behaviour, not a defect introduced here.
//
// LOAD MORE is drawn inline rather than through controls/QbzLoadMore.qml: that
// control ships exactly two shapes (plain centred text, and Search's bordered
// pill — its own header says "the arms are `bordered` and nothing else"), and
// Tauri's is a third, a filled surface-elevated 10/24 button. Under a visual
// 1:1 mandate the button wins over the reuse.
//
// ANIMATION: the only motion on this page is the card's 150ms hover lift, a
// gesture-bounded Behavior, which is the explicitly allowed class (the GPU
// pulse law, GPU-COST-INVESTIGATION.md §9, and cards/ArtistCard.qml:170,231).
// There is NO spinner and NO skeleton here — Tauri's loading state is a single
// line of muted text, which happens to also be the cheapest possible answer.

import QtQuick
import "../kiosk"
import com.blitzfc.qbz
import "../controls"
import "../theme"

Rectangle {
    id: root
    property bool kioskHost: false

    color: ambientOn ? "transparent" : theme.surfaceMain
    readonly property bool ambientOn: theme.ambientOn
    radius: 12

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // ONE document, re-parsed on every publish. `musicianRev` is a pure binding
    // dependency, exactly like the `rev` argument of QbzSession.tr(msgid, rev):
    // the parse ignores it, reading it is what makes this binding re-run when
    // Rust republishes a document that happens to be byte-identical.
    readonly property var doc: root._parse(QbzMusician.musicianJson,
                                           QbzMusician.musicianRev)
    function _parse(s, rev) {
        try {
            return JSON.parse(s)
        } catch (e) {
            return ({})
        }
    }

    readonly property var appearances: doc.appearances || []
    readonly property var bands: doc.bands || []
    readonly property int total: doc.total || 0

    // CSS `text-transform: capitalize` — title-case EVERY word, and leave the
    // rest of each word alone (it is not "uppercase the first letter"). Tauri
    // renders "lead guitar" as "Lead Guitar" and "member of" as "Member Of",
    // and the role string is user-visible in two places here plus every badge.
    // QML has no text-transform, so it happens on the string.
    function _capitalize(s) {
        if (!s)
            return ""
        return String(s).replace(/(^|\s)(\S)/g, function (m, sp, ch) {
            return sp + ch.toUpperCase()
        })
    }

    // Body-state precedence, in Tauri's own branch order (:143-204): loading,
    // then error, then empty, then the grid. `state` is the document's, so the
    // three are mutually exclusive by construction.
    readonly property bool isLoading: doc.state === "loading"
    readonly property bool isError: doc.state === "error"
    readonly property bool isEmpty: !isLoading && !isError && appearances.length === 0

    // The keyboard focus ring. Drawn by the control, not by the theme: nothing
    // in this tree had one before (grep: theme.focusRing had ONE consumer,
    // LogViewerModal.qml:244's search field), and a control that can take Tab
    // focus without showing it is worse than one that cannot take it at all.
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

    // ============================ offline gate ============================
    // Every page in this port carries it (ArtistReleasesView.qml:100-111 is the
    // model) and this one is 100% network-fed: offline it could only ever paint
    // its error state.
    QbzOfflinePlaceholder {
        visible: QbzSession.offline
        anchors.centerIn: parent
        showSettingsAction: true
        onSettingsClicked: QbzShell.navigateTo("settings")
    }

    Column {
        anchors.fill: parent
        spacing: 0
        visible: !QbzSession.offline

        // --- Sticky header (.page-header, :216-226) ------------------------
        // 24 top + 56 row + 16 bottom + the 1px rule = 97. Tauri makes it
        // `position: sticky`; here it is simply a fixed sibling above the
        // scroller, which is the same thing from scroll 0 onwards and is the
        // shape every fixed-header view in this port uses.
        //
        // NOT ported: `data-tauri-drag-region="deep"`. Window dragging in this
        // port belongs to the shell titlebar (HeaderBar), and no view header
        // installs a DragHandler of its own — adding one here would be a new
        // behaviour, not parity.
        Item {
            id: header
            width: parent.width
            height: root.kioskHost ? 72 : 97

            Rectangle {
                anchors.fill: parent
                // A full-bleed pane child at y=0 rounds ITSELF: Qt's `clip` is
                // a rectangular scissor that ignores `radius`, so the content
                // pane's own rounding cannot reach this bar (AppShell.qml says
                // "there is no mask that can do it for it here").
                topLeftRadius: theme.radiusMd
                topRightRadius: theme.radiusMd
                color: root.ambientOn ? theme.surfaceMainA30 : theme.surfaceMain
            }

            // .musician-header: flex, align-items center, gap 16.
            Row {
                x: 40
                y: 24
                height: 56
                width: Math.max(0, header.width - 40 - 24)
                spacing: 16

                // .musician-avatar — 56px circle, surface-elevated, muted User
                // glyph at 32. No photo branch exists: there is no image field.
                Rectangle {
                    width: 56
                    height: 56
                    radius: 28
                    color: theme.surfaceElevated
                    QbzIcon {
                        anchors.centerIn: parent
                        name: "user"
                        width: 32
                        height: root.kioskHost ? 44 : 32
                        tintName: "muted"
                    }
                }

                // .musician-info — column, gap 4.
                Column {
                    width: parent.width - 56 - 16
                    anchors.verticalCenter: parent.verticalCenter
                    spacing: 4

                    Text {
                        width: parent.width
                        text: root.doc.name || ""
                        color: theme.textPrimary
                        font.pixelSize: 24
                        font.weight: theme.weightBold
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        // SERVER TEXT, capitalized by CSS in the reference.
                        text: root._capitalize(root.doc.role || "")
                        color: theme.textMuted
                        font.pixelSize: 14
                        elide: Text.ElideRight
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

        // --- Scrolling content (.page-content, padding 24) -----------------
        Item {
            width: parent.width
            height: parent.height - header.height

            Flickable {
                id: flick
                anchors.fill: parent
                clip: true
                contentWidth: width
                contentHeight: page.implicitHeight
                boundsBehavior: Flickable.StopAtBounds

                Column {
                    id: page
                    width: parent.width
                    leftPadding: 24
                    rightPadding: 24
                    topPadding: 24
                    // Tauri's own padding is a flat 24; the extra 76 is this
                    // port's shared bottom inset so the last row clears the
                    // NowPlayingBar (every view uses 100).
                    bottomPadding: 100
                    spacing: 0

                    readonly property real contentWidth: width - 48

                    // ======== Section 1 — Bands & Projects (:112-132) ======
                    // DEAD in Tauri (`bands` is hardcoded empty on both return
                    // paths of the resolver, in BOTH backends) and made real
                    // here: DV-2 feeds it from the MusicBrainz relationship
                    // `groups`. The section is ABSENT, not empty, when there is
                    // nothing to show — no spinner, no empty frame, no error
                    // frame (00-CONTRACT §5.3's state table).
                    Column {
                        visible: root.bands.length > 0
                        width: page.contentWidth
                        spacing: 16
                        bottomPadding: 32

                        Text {
                            text: root.t("Bands & Projects")
                            color: theme.textPrimary
                            font.pixelSize: theme.fontSection
                            font.weight: theme.weightSemibold
                        }

                        // .bands-grid — flex-wrap, gap 10.
                        Flow {
                            visible: !root.kioskHost
                            width: parent.width
                            spacing: 10

                            Repeater {
                                model: root.kioskHost ? [] : root.bands
                                delegate: Item {
                                    id: bandCell
                                    required property var modelData
                                    // Inert-but-styled when the group did not
                                    // resolve to an MBID (00-CONTRACT §5.3):
                                    // Tauri's chip only console.logs a TODO, so
                                    // a dead chip is still 1:1 — a LIVE one is
                                    // the improvement, and it needs an id.
                                    readonly property bool live:
                                        (modelData.mbid || "") !== ""

                                    width: bandChip.width
                                    height: bandChip.height
                                    activeFocusOnTab: live
                                    Keys.onPressed: function (event) {
                                        if (bandCell.live
                                            && (event.key === Qt.Key_Return
                                                || event.key === Qt.Key_Enter
                                                || event.key === Qt.Key_Space)) {
                                            bandCell.open()
                                            event.accepted = true
                                        }
                                    }
                                    function open() {
                                        if (bandCell.live)
                                            QbzMusician.openBand(bandCell.modelData.mbid)
                                    }

                                    // .band-card — 10/16 padding, gap 10,
                                    // surface-elevated, 14px. Tauri's radius is
                                    // 20 (a pill); DV-19 drops it to Radius.sm.
                                    Rectangle {
                                        id: bandChip
                                        width: bandRow.width + 32
                                        height: bandRow.height + 20
                                        radius: theme.radiusSm
                                        color: bandArea.containsMouse
                                            ? theme.bgHover : theme.surfaceElevated

                                        Row {
                                            id: bandRow
                                            anchors.centerIn: parent
                                            spacing: 10

                                            QbzIcon {
                                                anchors.verticalCenter: parent.verticalCenter
                                                name: "music"
                                                width: 16
                                                height: 16
                                                tintName: "muted"
                                            }
                                            Text {
                                                anchors.verticalCenter: parent.verticalCenter
                                                text: bandCell.modelData.name || ""
                                                color: theme.textPrimary
                                                font.pixelSize: 14
                                                font.weight: theme.weightMedium
                                            }
                                        }

                                        MouseArea {
                                            id: bandArea
                                            anchors.fill: parent
                                            hoverEnabled: true
                                            cursorShape: bandCell.live
                                                ? Qt.PointingHandCursor : Qt.ArrowCursor
                                            onClicked: bandCell.open()
                                        }
                                    }

                                    FocusRing {
                                        host: bandCell
                                        radius: theme.radiusSm + 3
                                    }
                                }
                            }
                        }
                        Item {
                            id: kioskBands
                            visible: root.kioskHost
                            width: parent.width
                            height: visible ? root.bands.length*64 : 0
                            readonly property real viewTop: { var h=flick.contentHeight; return flick.contentY-mapToItem(flick.contentItem,0,0).y }
                            readonly property int first: Math.max(0,Math.floor(viewTop/64)-1)
                            readonly property int last: Math.max(first,Math.min(root.bands.length,Math.ceil((viewTop+flick.height)/64)+1))
                            Repeater {
                                model: root.kioskHost ? root.bands.slice(kioskBands.first,kioskBands.last) : []
                                delegate: SettingsButton {
                                    required property var modelData
                                    required property int index
                                    kioskHost: true
                                    y: (kioskBands.first+index)*64
                                    width: kioskBands.width
                                    btnHeight: 64
                                    text: modelData.name || ""
                                    enabled: (modelData.mbid || "") !== ""
                                    onClicked: QbzMusician.openBand(modelData.mbid)
                                }
                            }
                        }

                    }

                    // ======== Section 2 — Appears On (:135-205) ============
                    // spacing 0, and the title carries its own 16px bottom
                    // margin: the four bodies and the Load-more block are
                    // SIBLINGS of the grid in the reference, with no gap of
                    // their own beyond their padding (`.load-more` is
                    // `padding: 24px 0`, not 24 on top of a flex gap).
                    Column {
                        width: page.contentWidth
                        spacing: 0

                        // .section-title + the 14/400 muted count, margin
                        // 0 0 16.
                        Row {
                            spacing: 8
                            bottomPadding: 16

                            Text {
                                id: appearsTitle
                                text: root.t("Appears On")
                                color: theme.textPrimary
                                font.pixelSize: theme.fontSection
                                font.weight: theme.weightSemibold
                            }
                            // `.section-title` is `align-items: center`, so the
                            // count rides the title's vertical centre, not its
                            // baseline.
                            Text {
                                visible: root.total > 0
                                anchors.verticalCenter: appearsTitle.verticalCenter
                                text: "(" + root.total + ")"
                                color: theme.textMuted
                                font.pixelSize: 14
                            }
                        }

                        // --- State 1: loading (:143-146) ------------------
                        // Text only — the reference mounts no spinner here.
                        // The msgid is the catalogue's "Loading…" (U+2026), not
                        // Tauri's ASCII "Loading...": 00-CONTRACT §7 rules that
                        // an existing msgid is reused rather than adding a
                        // near-twin to eight catalogues over one glyph.
                        Item {
                            visible: root.isLoading
                            width: parent.width
                            height: 48 * 2 + loadingLbl.height

                            Text {
                                id: loadingLbl
                                anchors.centerIn: parent
                                text: root.t("Loading…")
                                color: theme.textMuted
                                font.pixelSize: 14
                            }
                        }

                        // --- State 2: error (:147-150) --------------------
                        // Tauri renders its `error` state variable, which it
                        // only ever sets to this one hardcoded English string.
                        // Here the document may carry a real message; the
                        // translated literal is the fallback so the box is
                        // never blank.
                        Item {
                            visible: root.isError
                            width: parent.width
                            height: 48 * 2 + errorLbl.height

                            Text {
                                id: errorLbl
                                anchors.centerIn: parent
                                width: parent.width
                                text: (root.doc.error || "") !== ""
                                    ? root.doc.error
                                    : root.t("Failed to load appearances")
                                color: theme.textMuted
                                font.pixelSize: 14
                                horizontalAlignment: Text.AlignHCenter
                                wrapMode: Text.WordWrap
                            }
                        }

                        // --- State 3: empty (:151-155) --------------------
                        // Disc glyph 32 over the line, gap 12, padding 48.
                        // Msgid reused from the catalogue WITH its trailing
                        // period (Tauri's has none) — §7 again.
                        Column {
                            visible: root.isEmpty
                            width: parent.width
                            topPadding: 48
                            bottomPadding: 48
                            spacing: 12

                            QbzIcon {
                                anchors.horizontalCenter: parent.horizontalCenter
                                name: "disc"
                                width: 32
                                height: root.kioskHost ? 44 : 32
                                tintName: "muted"
                            }
                            Text {
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: root.t("No album appearances found.")
                                color: theme.textMuted
                                font.pixelSize: 14
                            }
                        }

                        // --- The grid (:156-204) --------------------------
                        // `repeat(auto-fill, minmax(160px, 1fr))` with gap 20:
                        // as many 160px-minimum tracks as fit, then every track
                        // STRETCHES to share the row. So the column count and
                        // the cell width are computed, not fixed — this is not
                        // one of the port's fixed-pitch card grids.
                        Grid {
                            id: grid
                            visible: !root.kioskHost && !root.isLoading && !root.isError
                                     && root.appearances.length > 0
                            width: parent.width
                            spacing: 20
                            columns: Math.max(1, Math.floor((width + spacing)
                                                            / (160 + spacing)))
                            readonly property real cellW:
                                (width - (columns - 1) * spacing) / columns

                            Repeater {
                                model: root.kioskHost ? [] : root.appearances
                                delegate: Item {
                                    id: cell
                                    required property var modelData

                                    width: grid.cellW
                                    height: cardCol.implicitHeight
                                    activeFocusOnTab: true
                                    Keys.onPressed: function (event) {
                                        if (event.key === Qt.Key_Return
                                            || event.key === Qt.Key_Enter
                                            || event.key === Qt.Key_Space) {
                                            cell.open()
                                            event.accepted = true
                                        }
                                    }
                                    function open() {
                                        var id = cell.modelData.albumId || ""
                                        if (id !== "")
                                            QbzMusician.openAlbum(id)
                                    }

                                    // .album-card — column, gap 12, hover
                                    // translateY(-4px) over 150ms. A Behavior
                                    // bounded by a pointer gesture is the
                                    // allowed class under the pulse law; the
                                    // lift is on the CONTENT, not on the cell,
                                    // because the Grid owns the cell's y.
                                    Column {
                                        id: cardCol
                                        width: parent.width
                                        y: cardArea.containsMouse ? -4 : 0
                                        spacing: 12

                                        Behavior on y {
                                            NumberAnimation { duration: 150 }
                                        }

                                        // .album-artwork — 1:1, radius 8, on a
                                        // surface-elevated bed. NO shadow.
                                        Rectangle {
                                            width: parent.width
                                            height: width
                                            radius: theme.radiusSm
                                            color: theme.surfaceElevated
                                            clip: true

                                            // `artPath`, NOT `artworkUrl`.
                                            // The remote url is a Rust-side
                                            // key; the cover that exists on
                                            // disk is what an Image can draw,
                                            // and musician_qt.rs:100-109 says
                                            // in as many words that a delegate
                                            // binding `artworkUrl` into a
                                            // source "draws nothing". The
                                            // field is ADDITIVE to the frozen
                                            // §13.2 shape and its two-pass
                                            // fill needs no QML plumbing.
                                            RoundedImage {
                                                id: cover
                                                anchors.fill: parent
                                                source: cell.modelData.artPath || ""
                                                radius: theme.radiusSm
                                            }
                                            // The placeholder branch is LIVE,
                                            // not defensive: album_artwork is
                                            // `.unwrap_or_default()`, so an
                                            // empty cover is an expected value.
                                            // It doubles as the pre-download
                                            // state, which is what Tauri's
                                            // `<img loading="lazy">` shows
                                            // there anyway. `ready` is the
                                            // shared handover signal — true
                                            // only once THIS source is on
                                            // screen (RoundedImage.qml:247-259).
                                            QbzIcon {
                                                anchors.centerIn: parent
                                                visible: !cover.ready
                                                name: "disc"
                                                width: 24
                                                height: 24
                                                tintName: "muted"
                                            }
                                        }

                                        // .album-info — column, gap 4.
                                        Column {
                                            width: parent.width
                                            spacing: 4

                                            Text {
                                                width: parent.width
                                                text: cell.modelData.title || ""
                                                color: theme.textPrimary
                                                font.pixelSize: 14
                                                font.weight: theme.weightMedium
                                                elide: Text.ElideRight
                                            }
                                            Text {
                                                width: parent.width
                                                text: cell.modelData.artistName || ""
                                                color: theme.textMuted
                                                font.pixelSize: 12
                                                elide: Text.ElideRight
                                            }

                                            // .album-meta — row, gap 8,
                                            // margin-top 4.
                                            Row {
                                                width: parent.width
                                                topPadding: 4
                                                spacing: 8

                                                // The FULL ISO date, verbatim
                                                // (§5.4). Not a year.
                                                Text {
                                                    visible: (cell.modelData.releaseDate || "") !== ""
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    text: cell.modelData.releaseDate || ""
                                                    color: theme.textMuted
                                                    font.pixelSize: 11
                                                }

                                                // .role-badge — ALWAYS drawn,
                                                // 10px on 2/8 padding. Tauri's
                                                // radius 10 -> Radius.sm
                                                // (DV-19). Same string on every
                                                // card by design.
                                                Rectangle {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    width: badgeLbl.implicitWidth + 16
                                                    height: badgeLbl.implicitHeight + 4
                                                    radius: theme.radiusSm
                                                    color: theme.surfaceElevated

                                                    Text {
                                                        id: badgeLbl
                                                        anchors.centerIn: parent
                                                        text: root._capitalize(
                                                            cell.modelData.roleOnAlbum || "")
                                                        color: theme.textSecondary
                                                        font.pixelSize: 10
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    MouseArea {
                                        id: cardArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: cell.open()
                                    }

                                    FocusRing {
                                        host: cell
                                        radius: theme.radiusSm + 3
                                    }
                                }
                            }
                        }
                        Item {
                            id: kioskAppearances
                            visible: root.kioskHost && !root.isLoading && !root.isError && root.appearances.length>0
                            width: parent.width
                            readonly property int columns: Math.max(1,Math.floor((width+20)/180))
                            readonly property real cellW: (width-(columns-1)*20)/columns
                            readonly property real pitch: cellW+132
                            height: visible ? Math.ceil(root.appearances.length/columns)*pitch : 0
                            readonly property real viewTop: { var h=flick.contentHeight; return flick.contentY-mapToItem(flick.contentItem,0,0).y }
                            readonly property int first: Math.max(0,Math.floor(viewTop/pitch)-1)*columns
                            readonly property int last: Math.max(first,Math.min(root.appearances.length,(Math.ceil((viewTop+flick.height)/pitch)+1)*columns))
                            Repeater {
                                model: root.kioskHost ? root.appearances.slice(kioskAppearances.first,kioskAppearances.last) : []
                                delegate: Item {
                                    id: cell
                                    required property var modelData
                                    required property int index
                                    x: ((kioskAppearances.first+index)%kioskAppearances.columns)*(kioskAppearances.cellW+20)
                                    y: Math.floor((kioskAppearances.first+index)/kioskAppearances.columns)*kioskAppearances.pitch

                                    width: kioskAppearances.cellW
                                    height: cardCol.implicitHeight
                                    activeFocusOnTab: true
                                    Keys.onPressed: function (event) {
                                        if (event.key === Qt.Key_Return
                                            || event.key === Qt.Key_Enter
                                            || event.key === Qt.Key_Space) {
                                            cell.open()
                                            event.accepted = true
                                        }
                                    }
                                    function open() {
                                        var id = cell.modelData.albumId || ""
                                        if (id !== "")
                                            QbzMusician.openAlbum(id)
                                    }

                                    // .album-card — column, gap 12, hover
                                    // translateY(-4px) over 150ms. A Behavior
                                    // bounded by a pointer gesture is the
                                    // allowed class under the pulse law; the
                                    // lift is on the CONTENT, not on the cell,
                                    // because the Grid owns the cell's y.
                                    Column {
                                        id: cardCol
                                        width: parent.width
                                        y: cardArea.containsMouse ? -4 : 0
                                        spacing: 12

                                        Behavior on y {
                                            NumberAnimation { duration: 150 }
                                        }

                                        // .album-artwork — 1:1, radius 8, on a
                                        // surface-elevated bed. NO shadow.
                                        Rectangle {
                                            width: parent.width
                                            height: width
                                            radius: theme.radiusSm
                                            color: theme.surfaceElevated
                                            clip: true

                                            // `artPath`, NOT `artworkUrl`.
                                            // The remote url is a Rust-side
                                            // key; the cover that exists on
                                            // disk is what an Image can draw,
                                            // and musician_qt.rs:100-109 says
                                            // in as many words that a delegate
                                            // binding `artworkUrl` into a
                                            // source "draws nothing". The
                                            // field is ADDITIVE to the frozen
                                            // §13.2 shape and its two-pass
                                            // fill needs no QML plumbing.
                                            KioskArtwork {
                                                id: cover
                                                anchors.fill: parent
                                                source: cell.modelData.artPath || ""
                                                radius: theme.radiusSm
                                            }
                                            // The placeholder branch is LIVE,
                                            // not defensive: album_artwork is
                                            // `.unwrap_or_default()`, so an
                                            // empty cover is an expected value.
                                            // It doubles as the pre-download
                                            // state, which is what Tauri's
                                            // `<img loading="lazy">` shows
                                            // there anyway. `ready` is the
                                            // shared handover signal — true
                                            // only once THIS source is on
                                            // screen (RoundedImage.qml:247-259).
                                            QbzIcon {
                                                anchors.centerIn: parent
                                                visible: !cover.ready
                                                name: "disc"
                                                width: 24
                                                height: 24
                                                tintName: "muted"
                                            }
                                        }

                                        // .album-info — column, gap 4.
                                        Column {
                                            width: parent.width
                                            spacing: 4

                                            Text {
                                                width: parent.width
                                                text: cell.modelData.title || ""
                                                color: theme.textPrimary
                                                font.pixelSize: 14
                                                font.weight: theme.weightMedium
                                                elide: Text.ElideRight
                                            }
                                            Text {
                                                width: parent.width
                                                text: cell.modelData.artistName || ""
                                                color: theme.textMuted
                                                font.pixelSize: 12
                                                elide: Text.ElideRight
                                            }

                                            // .album-meta — row, gap 8,
                                            // margin-top 4.
                                            Row {
                                                width: parent.width
                                                topPadding: 4
                                                spacing: 8

                                                // The FULL ISO date, verbatim
                                                // (§5.4). Not a year.
                                                Text {
                                                    visible: (cell.modelData.releaseDate || "") !== ""
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    text: cell.modelData.releaseDate || ""
                                                    color: theme.textMuted
                                                    font.pixelSize: 11
                                                }

                                                // .role-badge — ALWAYS drawn,
                                                // 10px on 2/8 padding. Tauri's
                                                // radius 10 -> Radius.sm
                                                // (DV-19). Same string on every
                                                // card by design.
                                                Rectangle {
                                                    anchors.verticalCenter: parent.verticalCenter
                                                    width: badgeLbl.implicitWidth + 16
                                                    height: badgeLbl.implicitHeight + 4
                                                    radius: theme.radiusSm
                                                    color: theme.surfaceElevated

                                                    Text {
                                                        id: badgeLbl
                                                        anchors.centerIn: parent
                                                        text: root._capitalize(
                                                            cell.modelData.roleOnAlbum || "")
                                                        color: theme.textSecondary
                                                        font.pixelSize: 10
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    MouseArea {
                                        id: cardArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: cell.open()
                                    }

                                    FocusRing {
                                        host: cell
                                        radius: theme.radiusSm + 3
                                    }
                                }
                            }
                        }


                        // --- Load more (:193-203) ------------------------
                        // Only when hasMore. Disabled while a page is in
                        // flight, label swaps to the busy msgid.
                        Item {
                            visible: root.doc.hasMore === true
                                     && !root.isLoading && !root.isError
                            width: parent.width
                            height: 24 * 2 + 40

                            Item {
                                id: moreCell
                                anchors.centerIn: parent
                                width: moreBtn.width
                                height: moreBtn.height
                                activeFocusOnTab: root.doc.loadingMore !== true
                                Keys.onPressed: function (event) {
                                    if (root.doc.loadingMore !== true
                                        && (event.key === Qt.Key_Return
                                            || event.key === Qt.Key_Enter
                                            || event.key === Qt.Key_Space)) {
                                        QbzMusician.loadMore()
                                        event.accepted = true
                                    }
                                }

                                Rectangle {
                                    id: moreBtn
                                    width: moreLbl.implicitWidth + 48
                                    height: root.kioskHost ? 64 : moreLbl.implicitHeight + 20
                                    radius: theme.radiusSm
                                    opacity: root.doc.loadingMore === true ? 0.6 : 1.0
                                    color: moreArea.containsMouse
                                           && root.doc.loadingMore !== true
                                        ? theme.bgHover : theme.surfaceElevated

                                    Text {
                                        id: moreLbl
                                        anchors.centerIn: parent
                                        // Both msgids already exist in all
                                        // eight catalogues; Tauri's "Load More"
                                        // capital M is not worth a ninth entry
                                        // (§7).
                                        text: root.doc.loadingMore === true
                                            ? root.t("Loading…") : root.t("Load more")
                                        color: theme.textPrimary
                                        font.pixelSize: 14
                                        font.weight: theme.weightMedium
                                    }

                                    MouseArea {
                                        id: moreArea
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: root.doc.loadingMore === true
                                            ? Qt.ArrowCursor : Qt.PointingHandCursor
                                        onClicked: {
                                            if (root.doc.loadingMore !== true)
                                                QbzMusician.loadMore()
                                        }
                                    }
                                }

                                FocusRing {
                                    host: moreCell
                                    radius: theme.radiusSm + 3
                                }
                            }
                        }
                    }
                }
            }

            // Back/forward scroll memory (controls/ScrollMemory.qml): reports
            // this container's offset while it is the live page, and restores it
            // when a back/forward step arms this route.
            ScrollMemory { target: flick; scope: "musician" }
            QbzScrollBar {
                anchors.right: parent.right
                anchors.rightMargin: 4
                anchors.top: parent.top
                anchors.bottom: parent.bottom
                target: flick
            }
        }
    }
}
