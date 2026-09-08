// PurchaseListHeader — the column labels above the purchases list rows.
// Widths mirror PurchaseListRow (art 48 · stretch · quality · released ·
// purchased, 12px gaps, 8/12 margins) so each label sits on its column, the
// way views/AlbumListHeader.qml does for the album list.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Item {
    id: root

    QbzTheme { id: theme }

    function t(s) { return QbzSession.tr(s, QbzSession.trRev) }

    readonly property int colQuality: 150
    readonly property int colReleased: 120
    readonly property int colPurchased: 130

    width: parent ? parent.width : 0
    height: 28

    component ColLabel: Text {
        color: theme.textMuted
        font.pixelSize: 11
        font.weight: theme.weightSemibold
        font.letterSpacing: 0.5
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
        height: parent.height
    }

    Row {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 12
        spacing: 12
        Item { width: 48; height: 1 }
        ColLabel {
            width: Math.max(0, parent.width - 48
                - root.colQuality - root.colReleased - root.colPurchased - 4 * 12)
            text: root.t("Title").toUpperCase()
        }
        ColLabel { width: root.colQuality; text: root.t("Quality").toUpperCase() }
        ColLabel { width: root.colReleased; text: root.t("Release date").toUpperCase() }
        ColLabel { width: root.colPurchased; text: root.t("Purchased").toUpperCase() }
    }
}
