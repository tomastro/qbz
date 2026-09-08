// QualityInline — the compact one-line quality stamp shared by AudioStamp's
// first row and surfaces that need the same NPB Small treatment without its
// output-backend LEDs.
//
// The caller resolves the displayed tier/detail. That keeps AudioStamp's
// delivered-vs-source logic in one place while Album Quick View can show the
// album's catalog maximum through the exact same visual primitive.

import QtQuick
import QtQuick.Controls
import "../theme"

Item {
    id: root

    QbzTheme { id: theme }

    property int maxWidth: 150
    property string tierLabel: ""
    property string detail: ""
    property bool downgraded: false
    property string tooltipText: ""

    implicitWidth: Math.min(root.maxWidth, qualityLine.implicitWidth)
    implicitHeight: qualityLine.implicitHeight
    width: implicitWidth
    height: implicitHeight

    Row {
        id: qualityLine
        anchors.left: parent.left
        spacing: 5

        Text {
            id: tierText
            text: root.tierLabel
            font.pixelSize: 9
            font.weight: Font.Bold
            font.letterSpacing: 0.3
            color: theme.textSecondary
            anchors.verticalCenter: parent.verticalCenter
        }
        // Downgrade arrow (glyph, not a pill — ADR-008). Amber so it reads
        // as "adjusted", not as an error.
        Text {
            id: arrowText
            visible: root.downgraded
            text: "↓"
            font.pixelSize: 9
            font.weight: Font.Bold
            color: "#eab308"
            anchors.verticalCenter: parent.verticalCenter
        }
        Text {
            text: root.detail
            font.pixelSize: 9
            font.weight: Font.DemiBold
            color: theme.textPrimary
            elide: Text.ElideRight
            // Elide needs an explicit width. None of the sibling widths
            // depend on this value, so this cannot form a binding loop.
            width: Math.max(0, Math.min(implicitWidth,
                root.maxWidth - tierText.implicitWidth - 5
                    - (arrowText.visible ? arrowText.implicitWidth + 5 : 0)))
            anchors.verticalCenter: parent.verticalCenter
        }
    }

    MouseArea {
        id: qualityHover
        anchors.fill: parent
        hoverEnabled: true
    }

    // 1:1 with AudioStamp's former inline tooltip: the bubble bottom sits
    // 9px above the anchor and its caret occupies the final 5px.
    ToolTip {
        id: qualityTip
        visible: qualityHover.containsMouse && root.tooltipText !== ""
        text: root.tooltipText
        delay: 0
        timeout: -1
        // Geometry pinned to the text itself. The Popup's implicit sizing is
        // what drew the NPB bubble wider than its lines and shifted the text
        // (never reproduced in isolation — tracked as inherited debt), so
        // nothing is left for the style to add: zero paddings, insets and
        // margins, and width/height bound straight to the content's
        // implicit size. The Text carries the 9/5/10 bubble paddings.
        padding: 0
        leftPadding: 0
        rightPadding: 0
        topPadding: 0
        bottomPadding: 0
        leftInset: 0
        rightInset: 0
        topInset: 0
        bottomInset: 0
        margins: 0
        width: contentItem ? contentItem.implicitWidth : 0
        height: contentItem ? contentItem.implicitHeight : 0
        x: Math.round((root.width - width) / 2)
        y: -height - 4
        contentItem: Text {
            text: qualityTip.text
            color: theme.textPrimary
            font.pixelSize: 11
            font.weight: theme.weightMedium
            leftPadding: 9
            rightPadding: 9
            topPadding: 5
            bottomPadding: 10
        }
        background: Item {
            Rectangle {
                anchors.fill: parent
                anchors.bottomMargin: 5
                color: theme.surfaceElevated
                radius: 4
            }
            Rectangle {
                width: 7.07
                height: 7.07
                rotation: 45
                antialiasing: true
                color: theme.surfaceElevated
                x: Math.round((parent.width - width) / 2)
                y: parent.height - 5 - height / 2
            }
        }
    }
}
