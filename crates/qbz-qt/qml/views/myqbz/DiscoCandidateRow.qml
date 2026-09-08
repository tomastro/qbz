// One candidate row of the Discography Builder table — QML port of
// myqbz/DiscographyBuilderView.slint:248-416 (`CandidateRow`), carrying with
// it the three helpers Slint declares in the same file: `TypeOverride` +
// its popover (:62-240), `TypeColor` (:38-46) and `SourceIndicator` (:50-59).
//
// Slint keeps all four inside the view file; here the row is its own file so
// DiscoBuilderView.qml holds only the page frame (spec 01 §8.5), and the
// type-override menu stays with the button it belongs to — one call site.
//
// 7-COLUMN GRID, the alignment backbone (Slint :242-244):
//     28 / 56 / 72 / 1fr / 72 / 60 / 156, padding-l/r 8, spacing 10.
// The header row in DiscoBuilderView.qml inlines the SAME template — they are
// a MATCHED PAIR (the AlbumListHeader / AlbumListRow convention). Change both.
//
// `alt` (an alternate release inside a group) dims the row, swaps the col[0]
// checkbox for a connector + checkbox, and shrinks every text by 1px.
//
// HOVER is ORed across the MouseAreas this row owns, because a child
// MouseArea with `hoverEnabled` steals hover from the row-wide one — the same
// reason rows/TrackRow.qml:213 ORs its four areas. SelectCheck does not expose
// its internal area, so hovering the 13px checkbox itself does not lift the
// row; TrackRow lives with the identical limitation on its artist/album links.
//
// TYPE MENU: Slint hand-rolls a Rectangle plus a 4000x4000 backdrop TouchArea
// and a global `open-type-menu-key` because it has no popup that closes on
// outside-click (:114-127). Qt has one, so `QbzContextMenu` owns the open
// state and that global does NOT exist on the bridge (spec 01 §8.6). The
// popup is a delegate child and dies when this row is recycled out of the
// ListView — accepted (spec 01 §3.3.6): Slint's menu is a row child too.

import QtQuick
import com.blitzfc.qbz
import "../../controls"
import "../../theme"
import "../local"

Rectangle {
    id: root
    property bool kioskHost: false

    /// One `DiscoCandidate` (spec 02 §5.8): key, source, sourceItemId, title,
    /// year, tracks, releaseType, isOverridden, qualityTier, qualityDetail,
    /// checked. (`artUrl`/`artPath` are unused — the table has no art cell.)
    property var candidate: ({})
    /// The OWNING group's key — `toggleChecked` is keyed by (group, candidate).
    property string groupKey: ""
    /// Alternate row (rendered under its group's primary).
    property bool alt: false
    /// Whether the OWNING group is a compilation. Slint :253-255: the
    /// "compilation" tag is driven by the GROUP flag, not by the candidate's
    /// effective release type.
    property bool groupIsCompilation: false
    /// Resting opacity, lifted to 1.0 on hover (:263). The parent passes it:
    /// primary = `groupIsCompilation ? 0.65 : 1.0`, alternate = 0.72
    /// unconditionally (:713,720 — an alternate inside a compilation group is
    /// still 0.72).
    property real restOpacity: 1.0

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    // --- Column geometry (the matched pair — see the header note) ----------
    readonly property int padH: 8
    readonly property int colGap: 10
    readonly property int colCheck: root.kioskHost ? 60 : 28
    readonly property int colYear: 56
    readonly property int colType: 72
    readonly property int colTracks: 72
    readonly property int colSource: 60
    readonly property int colQuality: 156
    // 28 + 56 + 72 + 72 + 60 + 156 = 444 fixed, 6 gaps of 10 = 60.
    readonly property real titleWidth: Math.max(
        0, root.width - 2 * root.padH - 444 - (root.kioskHost ? 32 : 0) - 6 * root.colGap)

    readonly property string releaseType: root.candidate.releaseType || ""
    readonly property bool isOverridden: root.candidate.isOverridden === true
    readonly property bool hovered: rowArea.containsMouse || titleArea.containsMouse
        || typeArea.containsMouse

    /// Per-release-type tint (Slint `DiscoTint`, :30-35 / `TypeColor` :38-46).
    /// The ONE legitimately-hardcoded palette in this view — spec 01 §2 keeps
    /// these four as literals ("the spec intends fixed non-token colours");
    /// plain "album" falls through to the muted token.
    function typeColor(kind) {
        return kind === "compilation" ? "#f59e0b"
            : kind === "live" ? "#e91e63"
            : kind === "ep" ? "#60a5fa"
            : kind === "single" ? "#a78bfa"
            : theme.textMuted
    }

    /// Menu rows in Slint's order (:142-147). Built by a function that TAKES
    /// `rev` so the array rebuilds on a live language switch.
    function typeChoices(rev) {
        return [
            { "id": "album", "label": QbzSession.tr("Album", rev) },
            { "id": "ep", "label": QbzSession.tr("EP", rev) },
            { "id": "single", "label": QbzSession.tr("Single", rev) },
            { "id": "live", "label": QbzSession.tr("Live", rev) },
            { "id": "compilation", "label": QbzSession.tr("Compilation", rev) }
        ]
    }

    height: root.kioskHost ? 64 : 44
    radius: 6
    color: root.hovered ? theme.surfaceHover : "transparent"
    opacity: root.hovered ? 1.0 : root.restOpacity

    // The row's own area does NOTHING but drive the hover lift (:265 —
    // `mouse-cursor: default`, no handler). Declared BEFORE the cells so
    // every interactive child sits above it.
    MouseArea {
        id: rowArea
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.ArrowCursor
    }

    // Bottom hairline, so adjacent rows (and their quality badges) read as
    // separated instead of stuck together (:269-273).
    Rectangle {
        y: parent.height - 1
        width: parent.width
        height: 1
        color: theme.borderSubtle
    }

    Row {
        anchors.fill: parent
        anchors.leftMargin: root.padH
        anchors.rightMargin: root.padH
        spacing: root.colGap

        // [0] 28px — checkbox (alternate: connector + checkbox).
        Item {
            width: root.colCheck
            height: parent.height
            Row {
                anchors.left: parent.left
                height: parent.height
                spacing: 3
                Text {
                    visible: root.alt
                    anchors.verticalCenter: parent.verticalCenter
                    // Untranslated in Slint too (:286) — a glyph, not copy.
                    text: "↳"
                    color: theme.textMuted
                    font.pixelSize: 12
                }
                SelectCheck {
                    diameter: root.kioskHost ? 44 : 13
                    anchors.verticalCenter: parent.verticalCenter
                    on: root.candidate.checked === true
                    onToggled: QbzDisco.toggleChecked(root.groupKey,
                                                      root.candidate.key || "")
                }
            }
        }

        // [1] 56px — year.
        Text {
            width: root.colYear
            height: parent.height
            text: root.candidate.year || ""
            color: root.alt ? theme.textMuted : theme.textSecondary
            font.pixelSize: root.alt ? 12 : 13
            horizontalAlignment: Text.AlignLeft
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }

        // [2] 72px — release-type override button + popover.
        Item {
            width: root.colType
            height: parent.height

            Rectangle {
                id: typeBtn
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                // Slint sizes the wrapper to the button's preferred size
                // (:71-72) and the button is a 4/2-padded label (:79-83).
                implicitWidth: typeLabel.implicitWidth + 8
                implicitHeight: typeLabel.implicitHeight + 4
                width: implicitWidth
                height: implicitHeight
                radius: 4
                color: typeArea.containsMouse ? theme.surfaceHover : "transparent"

                Text {
                    id: typeLabel
                    x: 4
                    y: 2
                    text: root.releaseType
                    color: root.typeColor(root.releaseType)
                    font.pixelSize: root.alt ? 10 : 11
                    font.weight: theme.weightSemibold
                    font.letterSpacing: 0.4
                }
                // Active-override marker. Slint has no text-decoration and
                // draws a 1px rule under the label (:96-103); QML's
                // font.underline exists but the rule IS the reference shape.
                Rectangle {
                    visible: root.isOverridden
                    x: typeLabel.x
                    y: parent.height - 3
                    width: typeLabel.width
                    height: 1
                    color: root.typeColor(root.releaseType)
                    opacity: 0.6
                }
                MouseArea {
                    id: typeArea
                    anchors.fill: parent
                    anchors.margins: -4   // the .type-btn `margin: 0 -4px` bleed
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: {
                        // CloseOnPressOutside fires BEFORE this handler, so
                        // without the guard a click on an open menu's trigger
                        // closes and immediately reopens it (spec 01 §3.3.5).
                        if (typeMenu.opened) {
                            typeMenu.close()
                            return
                        }
                        typeMenu.openBelowLeft(typeBtn)
                    }
                }
            }

            // Slint: `x: 0; y: parent.height + 4`, 160px (:128-131) — LEFT
            // aligned, hence openBelowLeft (openBelowRight would push a 160px
            // panel ~120px left of this ~40px button).
            QbzContextMenu {
                id: typeMenu
                menuWidth: 160
                // The component hardcodes padding 5 / surfaceMain / r8 /
                // borderMuted; this panel is 4 / surfaceCard / r6 /
                // surfaceElevated (:132-135), so both are overridden here
                // (spec 01 §3.3.1-2).
                padding: 4
                background: Rectangle {
                    color: theme.surfaceCard
                    radius: 6
                    border.width: 1
                    border.color: theme.surfaceElevated
                }

                Repeater {
                    model: root.typeChoices(QbzSession.trRev)
                    delegate: Rectangle {
                        id: choiceRow
                        required property var modelData
                        readonly property bool current:
                            root.releaseType === choiceRow.modelData.id
                        width: parent ? parent.width : 0
                        height: root.kioskHost ? 44 : 28
                        radius: 4
                        color: choiceArea.containsMouse ? theme.surfaceHover : "transparent"
                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 8
                            anchors.rightMargin: 8
                            spacing: 8
                            // Always a 12x12 leading slot so the labels stay
                            // aligned (:157-171).
                            Item {
                                width: 12
                                height: parent.height
                                QbzIcon {
                                    visible: choiceRow.current
                                    anchors.centerIn: parent
                                    name: "check"
                                    width: 12
                                    height: 12
                                    tintName: "accent"
                                }
                            }
                            Text {
                                width: Math.max(0, parent.width - 20)
                                height: parent.height
                                text: choiceRow.modelData.label
                                color: choiceRow.current ? theme.accent : theme.textPrimary
                                font.pixelSize: 12
                                verticalAlignment: Text.AlignVCenter
                                elide: Text.ElideRight
                            }
                        }
                        MouseArea {
                            id: choiceArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                typeMenu.close()
                                QbzDisco.setTypeOverride(
                                    root.candidate.source || "",
                                    root.candidate.sourceItemId || "",
                                    choiceRow.modelData.id)
                            }
                        }
                    }
                }
                // Reset rows — only while an override is active (:193-236):
                // a 9px band carrying a 1px rule at y=4, then a 28px row.
                Item {
                    visible: root.isOverridden
                    width: parent ? parent.width : 0
                    height: visible ? 9 : 0
                    Rectangle {
                        y: 4
                        width: parent.width
                        height: 1
                        color: theme.surfaceElevated
                    }
                }
                Rectangle {
                    visible: root.isOverridden
                    width: parent ? parent.width : 0
                    height: visible ? (root.kioskHost ? 44 : 28) : 0
                    radius: 4
                    color: resetArea.containsMouse ? theme.surfaceHover : "transparent"
                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: 8
                        anchors.rightMargin: 8
                        spacing: 8
                        Item {
                            width: 12
                            height: parent.height
                            QbzIcon {
                                anchors.centerIn: parent
                                name: "rotate-ccw"
                                width: 12
                                height: 12
                                tintName: "muted"
                            }
                        }
                        Text {
                            width: Math.max(0, parent.width - 20)
                            height: parent.height
                            text: root.t("Use auto-detected")
                            color: theme.textMuted
                            font.pixelSize: 12
                            verticalAlignment: Text.AlignVCenter
                            elide: Text.ElideRight
                        }
                    }
                    MouseArea {
                        id: resetArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            typeMenu.close()
                            QbzDisco.resetTypeOverride(
                                root.candidate.source || "",
                                root.candidate.sourceItemId || "")
                        }
                    }
                }
            }
        }

        // [3] 1fr — title (flexes) + the italic tag.
        Item {
            width: root.titleWidth
            height: parent.height
            Row {
                anchors.fill: parent
                spacing: 8
                Item {
                    width: Math.max(0, parent.width
                        - (rowTag.visible ? rowTag.implicitWidth + 8 : 0))
                    height: parent.height
                    Text {
                        anchors.fill: parent
                        text: root.candidate.title || ""
                        color: titleArea.containsMouse
                            ? theme.accent
                            : (root.alt ? theme.textMuted : theme.textPrimary)
                        font.pixelSize: root.alt ? 13 : 14
                        elide: Text.ElideRight
                        verticalAlignment: Text.AlignVCenter
                    }
                    MouseArea {
                        id: titleArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: QbzDisco.openAlbum(root.candidate.source || "",
                                                      root.candidate.sourceItemId || "")
                    }
                }
                // Alternates always show "alternate"; a primary shows
                // "compilation" only when its GROUP is one (:358-375).
                Text {
                    id: rowTag
                    visible: root.alt || root.groupIsCompilation
                    anchors.verticalCenter: parent.verticalCenter
                    text: root.alt ? root.t("alternate") : root.t("compilation")
                    color: theme.textMuted
                    font.pixelSize: 10
                    font.weight: theme.weightMedium
                    font.italic: true
                    font.letterSpacing: 0.8
                }
            }
        }

        // [4] 72px — tracks (centred).
        Text {
            width: root.colTracks
            height: parent.height
            text: root.candidate.tracks || ""
            color: root.alt ? theme.textMuted : theme.textSecondary
            font.pixelSize: root.alt ? 12 : 13
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }

        // [5] 60px — source glyph, centred in a 28x28 cell (`SourceIndicator`).
        Item {
            width: root.colSource
            height: parent.height
            Item {
                anchors.centerIn: parent
                width: 28
                height: root.kioskHost ? 44 : 28
                SourceIcon {
                    anchors.centerIn: parent
                    kind: root.candidate.source || ""
                    // A dense ROW: the media marks draw monochrome and tinted, like the
                    // hard-drive beside them. Colour logos are for cards — a list of
                    // them fights the text it labels.
                    mono: true
                    // Row glyph — SourceGlyph.slint:31: 15px, and the three
                    // Qobuz kinds one pixel larger (the wordmark's
                    // proportions). Explicit since the size rule became a
                    // per-arm knob; it used to be a hardcoded +2.
                    glyphSize: 15
                    qobuzSize: 16
                    localTint: "muted"
                }
            }
        }

        // [6] 156px — quality badge (implicitWidth floors at 136, so it fits).
        Item {
            width: root.colQuality
            height: parent.height
            QualityBadgeFull {
                anchors.centerIn: parent
                tier: root.candidate.qualityTier || ""
                detail: root.candidate.qualityDetail || ""
            }
        }
    }
}
