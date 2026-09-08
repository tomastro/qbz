// TrackInfoBody — the scrolling content of the Track Info modal (the
// `card-body` VerticalLayout of crates/qbz-ui/ui/album/TrackInfoModal.slint),
// split out of TrackInfoModal.qml for the size rule. `host` is the modal: it
// owns the parsed document, the close() and the nav actions.
//
// Sections, verbatim from the .slint:
//   loading  -> 60px padding all round, centered muted body text
//   error    -> 60px padding, 8px gap, headline + the error string (legal)
//   loaded   -> header (24 L/R, 16 T/B, 16 gap, 32x32 close X with an 18px
//               glyph) then the content block (24 L/R, 24 bottom):
//               metadata rows 24 apart in three equal columns · credits
//               behind 20 / 1px surface-elevated rule / 20, TWO independent
//               columns 24 apart with 20 between cells · copyright behind the
//               same 20 / rule / 20, 12px muted, wrapped.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Column {
    id: body
    /// The TrackInfoModal that owns the data + actions.
    required property var host

    QbzTheme { id: theme }

    /// Legibility mode for the immersive split panel (2026-08-31 visual
    /// cleanup): there the body sits directly on the ambient field, where
    /// theme tokens have no contrast guarantee over a light cover. On, every
    /// text uses a fixed light color plus the restrained native shadow the
    /// lyrics surfaces use. The desktop modal keeps the theme (default off).
    property bool overAmbient: false
    readonly property color cPrimary: overAmbient ? "#f2ffffff" : theme.textPrimary
    readonly property color cMuted: overAmbient ? "#b3ffffff" : theme.textMuted
    readonly property color cRule: overAmbient ? "#2effffff" : theme.surfaceElevated
    readonly property int cStyle: overAmbient ? Text.Raised : Text.Normal
    readonly property color cShadow: "#b0000000"
    // Links follow the ALBUM palette over ambient — the theme accent has no
    // contrast guarantee there (owner, 2026-08-31 round 2).
    AmbientAccent { id: bodyAmbientAccent }
    readonly property color cAccent: overAmbient ? bodyAmbientAccent.value : theme.accent

    readonly property var doc: host ? host.doc : ({})

    // ---- Loading ---------------------------------------------------------
    Item {
        visible: body.host && body.host.loading && body.host.errorText === ""
        width: parent.width
        height: visible ? 120 + loadingText.implicitHeight : 0
        Text {
            id: loadingText
            anchors.centerIn: parent
            width: Math.max(0, parent.width - 120)
            text: QbzSession.tr("Loading track info...", QbzSession.trRev)
            color: body.cMuted
            style: body.cStyle
            styleColor: body.cShadow
            font.pixelSize: theme.fontBody
            horizontalAlignment: Text.AlignHCenter
        }
    }

    // ---- Error -----------------------------------------------------------
    Item {
        visible: body.host && body.host.errorText !== ""
        width: parent.width
        height: visible ? 120 + errCol.implicitHeight : 0
        Column {
            id: errCol
            anchors.centerIn: parent
            width: Math.max(0, parent.width - 120)
            spacing: 8
            Text {
                width: parent.width
                text: QbzSession.tr("Failed to load track info", QbzSession.trRev)
                color: body.cMuted
                style: body.cStyle
                styleColor: body.cShadow
                font.pixelSize: theme.fontBody
                horizontalAlignment: Text.AlignHCenter
            }
            Text {
                width: parent.width
                text: body.host ? body.host.errorText : ""
                color: body.cMuted
                style: body.cStyle
                styleColor: body.cShadow
                font.pixelSize: theme.fontLegal
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
            }
        }
    }

    // ---- Loaded ----------------------------------------------------------
    Column {
        id: loadedCol
        visible: body.host && !body.host.loading && body.host.errorText === ""
        width: parent.width

        // Fixed credits-column width: card minus the 24px L/R content padding
        // (48) minus the 24px inter-column gutter, halved. Pinning each credit
        // cell to this keeps the right column aligned on every row.
        readonly property int contentW: Math.max(0, width - 48)
        readonly property int creditsColW: Math.max(0, (contentW - 24) / 2)
        readonly property int metaColW: Math.max(0, (contentW - 48) / 3)

        // --- Header: title / album / artist + close X ---------------------
        Item {
            width: parent.width
            height: Math.max(headCol.implicitHeight, 32) + 32

            Column {
                id: headCol
                x: 24
                y: 16
                // 24 L + 24 R padding, 16 gap, 32 close X.
                width: Math.max(0, parent.width - 24 - 24 - 16 - 32)
                spacing: 4

                Text {
                    width: parent.width
                    text: body.doc.title || ""
                    color: body.cPrimary
                    style: body.cStyle
                    styleColor: body.cShadow
                    // The immersive panel reads at a distance — its header
                    // steps up (owner, 2026-08-31 round 2).
                    font.pixelSize: body.overAmbient ? 24 : 16
                    font.weight: theme.weightSemibold
                    wrapMode: Text.WordWrap
                }
                Text {
                    visible: (body.doc.album || "") !== ""
                    width: parent.width
                    text: body.doc.album || ""
                    color: body.cMuted
                    style: body.cStyle
                    styleColor: body.cShadow
                    font.pixelSize: body.overAmbient ? 15 : theme.fontLegal
                    elide: Text.ElideRight
                }
                // Artist — a link only when an id exists.
                Item {
                    visible: (body.doc.artist || "") !== ""
                    width: parent.width
                    height: visible ? artistText.implicitHeight : 0
                    Text {
                        id: artistText
                        text: body.doc.artist || ""
                        color: (body.doc.artistId || "") !== ""
                            ? body.cAccent : body.cPrimary
                        style: body.cStyle
                        styleColor: body.cShadow
                        font.pixelSize: body.overAmbient ? 20 : 16
                        font.weight: theme.weightSemibold
                    }
                    MouseArea {
                        width: Math.min(artistText.implicitWidth, parent.width)
                        height: artistText.implicitHeight
                        cursorShape: (body.doc.artistId || "") !== ""
                            ? Qt.PointingHandCursor : Qt.ArrowCursor
                        onClicked: body.host.openArtist(body.doc.artistId || "")
                    }
                }
            }

            // Close X — the Slint is a bare 32x32 touch area with an 18px
            // glyph, text-muted -> text-primary, with NO hover fill;
            // QbzIconButton always paints one and idles at `secondary`, so
            // this one stays hand-rolled to keep the port 1:1.
            Item {
                width: 32
                height: 32
                x: parent.width - 24 - 32
                y: 16
                QbzIcon {
                    name: "x"
                    width: 18
                    height: 18
                    anchors.centerIn: parent
                    tintName: body.overAmbient
                        ? (closeArea.containsMouse ? "white" : "muted")
                        : (closeArea.containsMouse ? "textPrimary" : "muted")
                }
                MouseArea {
                    id: closeArea
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: body.host.close()
                }
            }
        }

        // --- Content block (24 L/R, 24 bottom) ----------------------------
        Item {
            width: parent.width
            height: contentCol.implicitHeight + 24

            Column {
                id: contentCol
                x: 24
                width: loadedCol.contentW

                // Metadata: fixed 3-column rows.
                Row {
                    width: parent.width
                    spacing: 24
                    InfoMetaCell {
                        cellWidth: loadedCol.metaColW
                        overAmbient: body.overAmbient
                        label: QbzSession.tr("Duration", QbzSession.trRev)
                        Text {
                            width: loadedCol.metaColW
                            text: body.doc.duration || ""
                            color: body.cPrimary
                            style: body.cStyle
                            styleColor: body.cShadow
                            font.pixelSize: 14
                            elide: Text.ElideRight
                        }
                    }
                    InfoMetaCell {
                        cellWidth: loadedCol.metaColW
                        overAmbient: body.overAmbient
                        label: QbzSession.tr("Quality", QbzSession.trRev)
                        Text {
                            width: loadedCol.metaColW
                            text: body.doc.quality || ""
                            color: body.cPrimary
                            style: body.cStyle
                            styleColor: body.cShadow
                            font.pixelSize: 14
                            elide: Text.ElideRight
                        }
                    }
                    // ISRC — the cell is dropped (not blanked) when absent;
                    // the empty third keeps the columns fixed.
                    InfoMetaCell {
                        visible: (body.doc.isrc || "") !== ""
                        cellWidth: loadedCol.metaColW
                        overAmbient: body.overAmbient
                        label: "ISRC"
                        Text {
                            width: loadedCol.metaColW
                            text: body.doc.isrc || ""
                            color: body.cMuted
                            style: body.cStyle
                            styleColor: body.cShadow
                            font.pixelSize: 14
                            elide: Text.ElideRight
                        }
                    }
                    Item {
                        visible: (body.doc.isrc || "") === ""
                        width: loadedCol.metaColW
                        height: 1
                    }
                }
                Item {
                    visible: (body.doc.label || "") !== ""
                    width: 1
                    height: 24
                }
                Row {
                    visible: (body.doc.label || "") !== ""
                    width: parent.width
                    spacing: 24
                    InfoMetaCell {
                        cellWidth: loadedCol.metaColW
                        overAmbient: body.overAmbient
                        label: QbzSession.tr("Label", QbzSession.trRev)
                        Item {
                            width: loadedCol.metaColW
                            height: labelText.implicitHeight
                            Text {
                                id: labelText
                                width: loadedCol.metaColW
                                text: body.doc.label || ""
                                color: (labelArea.containsMouse
                                        && (body.doc.labelId || "") !== "")
                                    ? body.cAccent : body.cPrimary
                                style: body.cStyle
                                styleColor: body.cShadow
                                font.pixelSize: 14
                                elide: Text.ElideRight
                            }
                            MouseArea {
                                id: labelArea
                                width: Math.min(labelText.implicitWidth, loadedCol.metaColW)
                                height: labelText.implicitHeight
                                hoverEnabled: true
                                cursorShape: (body.doc.labelId || "") !== ""
                                    ? Qt.PointingHandCursor : Qt.ArrowCursor
                                onClicked: body.host.openLabel(body.doc.labelId || "")
                            }
                        }
                    }
                }

                // --- Credits: two INDEPENDENT columns ---------------------
                Item {
                    visible: body.host && body.host.credits.length > 0
                    width: 1
                    height: 20
                }
                Rectangle {
                    visible: body.host && body.host.credits.length > 0
                    width: parent.width
                    height: 1
                    color: body.cRule
                }
                Item {
                    visible: body.host && body.host.credits.length > 0
                    width: 1
                    height: 20
                }
                Row {
                    visible: body.host && body.host.credits.length > 0
                    width: parent.width
                    spacing: 24
                    Column {
                        width: loadedCol.creditsColW
                        spacing: 20
                        Repeater {
                            model: body.host ? body.host.creditsLeft : []
                            delegate: InfoCreditCell {
                                required property var modelData
                                colW: loadedCol.creditsColW
                                overAmbient: body.overAmbient
                                accentColor: body.cAccent
                                cell: modelData
                                onNameClicked: function (n, r) { body.host.openMusician(n, r) }
                            }
                        }
                    }
                    Column {
                        width: loadedCol.creditsColW
                        spacing: 20
                        Repeater {
                            model: body.host ? body.host.creditsRight : []
                            delegate: InfoCreditCell {
                                required property var modelData
                                colW: loadedCol.creditsColW
                                overAmbient: body.overAmbient
                                accentColor: body.cAccent
                                cell: modelData
                                onNameClicked: function (n, r) { body.host.openMusician(n, r) }
                            }
                        }
                    }
                }

                // --- Copyright --------------------------------------------
                Item {
                    visible: (body.doc.copyright || "") !== ""
                    width: 1
                    height: 20
                }
                Rectangle {
                    visible: (body.doc.copyright || "") !== ""
                    width: parent.width
                    height: 1
                    color: body.cRule
                }
                Item {
                    visible: (body.doc.copyright || "") !== ""
                    width: 1
                    height: 20
                }
                Text {
                    visible: (body.doc.copyright || "") !== ""
                    width: parent.width
                    text: body.doc.copyright || ""
                    color: body.cMuted
                    style: body.cStyle
                    styleColor: body.cShadow
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                }
            }
        }
    }
}
