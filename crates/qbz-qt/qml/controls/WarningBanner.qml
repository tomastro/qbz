// WarningBanner (primitives/WarningBanner.slint) — a colored info / warning /
// error notice: 10%-alpha background, 30%-alpha border, a tone glyph, and a
// wrapping title + body (either may be empty).
//
// POC-NOTE (icon set): the Slint uses triangle-alert / circle-alert /
// circle-check-big glyphs; this port's icon set has none of them, so every
// variant uses `info` in the variant's tint. The color + the title still
// carry the tone.

import QtQuick
import "../theme"

Rectangle {
    id: root

    property string title: ""
    property string body: ""
    // "info" | "warning" | "error" | "success"
    property string variant: "warning"

    QbzTheme { id: theme }

    readonly property color tone: variant === "error" ? theme.danger
        : variant === "info" ? theme.accent
        : variant === "success" ? theme.success : theme.warning
    readonly property string toneTint: variant === "error" ? "favorite"
        : variant === "info" ? "accent"
        : variant === "success" ? "accent" : "warning"

    width: parent ? parent.width : 0
    height: row.implicitHeight + 24
    radius: 8
    color: Qt.rgba(tone.r, tone.g, tone.b, 0.10)
    border.width: 1
    border.color: Qt.rgba(tone.r, tone.g, tone.b, 0.30)

    Row {
        id: row
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 12
        spacing: 10

        QbzIcon {
            name: "info"
            tintName: root.toneTint
            width: 16
            height: 16
        }
        Column {
            width: parent.width - 26
            spacing: 3
            Text {
                visible: root.title !== ""
                width: parent.width
                text: root.title
                color: root.tone
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
                wrapMode: Text.WordWrap
            }
            Text {
                visible: root.body !== ""
                width: parent.width
                text: root.body
                color: theme.textSecondary
                font.pixelSize: theme.fontLegal
                wrapMode: Text.WordWrap
            }
        }
    }
}
