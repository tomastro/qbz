// Lyrics display-settings flyout — QML port of
// crates/qbz-ui/ui/shell/LyricsControlsFlyout.slint (`LyricsControlsPanel`).
//
// Row order is the reference's exactly: Auto-follow / Font / Size /
// Translation language / Active color / Uppercase / Lite fill / Dimming /
// footer (Copy lyrics + Reset). Selects instead of segmented pill rows
// (ADR-008), and the active color is a fixed swatch palette because there is
// no native color input (deviation D6) — the "Theme" reset link shows only
// while a custom color is set.
//
// Every mutation writes the QbzLyrics pref property directly (live preview)
// and fires `prefsChanged()` so Rust persists it into the SAME per-user
// lyrics_prefs.json the Slint build reads. Changing the translation language
// while the translation is ON re-requests the track in the new language
// (Rust side) — the row is never a dead control.
//
// This component is the popup CONTENT; the Popup shell lives at the anchor
// site in LyricsPanel.

import QtQuick
import com.blitzfc.qbz
import "../theme"
import "../controls"

Rectangle {
    id: panel

    // Copy gate — ready && lines > 0 (the reference's `can-copy`).
    property bool canCopy: false

    readonly property int rowWidth: 232        // 260 - 2 * 14 padding
    readonly property var swatches: [
        "#8b5cf6", "#ec4899", "#ef4444", "#f97316",
        "#eab308", "#22c55e", "#06b6d4", "#3b82f6"
    ]

    implicitWidth: 260
    implicitHeight: column.implicitHeight
    color: theme.surfaceMain
    radius: theme.radiusSm
    border.width: 1
    border.color: theme.borderMuted

    QbzTheme { id: theme }

    // Clipboard seam: this shell has no Rust clipboard bridge, so the copy
    // goes through the standard QML route (a hidden TextEdit's copy()).
    TextEdit {
        id: clipboard
        visible: false
        width: 0
        height: 0
    }

    function copyLyrics() {
        var text = QbzLyrics.plainText()
        if (text === "")
            return
        clipboard.text = text
        clipboard.selectAll()
        clipboard.copy()
        clipboard.text = ""
        QbzLyrics.notice = QbzSession.tr("Lyrics copied", QbzSession.trRev)
    }

    Column {
        id: column
        width: parent.width
        padding: 14
        spacing: 12

        // --- Auto-follow ------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 22
            Text {
                anchors.left: parent.left
                anchors.right: afToggle.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Auto-follow", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzToggle {
                id: afToggle
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                checked: QbzLyrics.autoFollow
                onToggled: function (value) {
                    QbzLyrics.autoFollow = value
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Font --------------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 34
            Text {
                anchors.left: parent.left
                anchors.right: fontSelect.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Font", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzSelect {
                id: fontSelect
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                menuWidth: 150
                options: [
                    QbzSession.tr("System", QbzSession.trRev),
                    "LINE Seed JP",
                    "Montserrat",
                    "Noto Sans",
                    "Source Sans 3"
                ]
                currentIndex: QbzLyrics.fontIndex
                onSelected: function (i) {
                    QbzLyrics.fontIndex = i
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Size --------------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 34
            Text {
                anchors.left: parent.left
                anchors.right: sizeSelect.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Size", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzSelect {
                id: sizeSelect
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                menuWidth: 110
                options: ["S", "M", "L", "XL"]
                currentIndex: QbzLyrics.sizeIndex
                onSelected: function (i) {
                    QbzLyrics.sizeIndex = i
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Translation language (Qobuz v10) ----------------------------
        Item {
            width: panel.rowWidth
            height: 34
            Text {
                anchors.left: parent.left
                anchors.right: langSelect.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Translation language", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzSelect {
                id: langSelect
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                menuWidth: 118
                popupWidth: 178
                options: [
                    QbzSession.tr("Auto (account language)", QbzSession.trRev),
                    "English", "Español", "Français", "Deutsch", "Italiano",
                    "Português", "Nederlands", "日本語", "Русский"
                ]
                currentIndex: QbzLyrics.translationLanguageIndex
                onSelected: function (i) {
                    QbzLyrics.translationLanguageIndex = i
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Active color -------------------------------------------------
        Column {
            width: panel.rowWidth
            spacing: 8
            Item {
                width: parent.width
                height: 18
                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: QbzSession.tr("Active color", QbzSession.trRev)
                    color: theme.textSecondary
                    font.pixelSize: 12
                }
                Text {
                    visible: QbzLyrics.useCustomColor
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: QbzSession.tr("Theme", QbzSession.trRev)
                    color: themeLinkArea.containsMouse ? theme.accentHover : theme.accent
                    font.pixelSize: 12
                    MouseArea {
                        id: themeLinkArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: {
                            QbzLyrics.useCustomColor = false
                            QbzLyrics.prefsChanged()
                        }
                    }
                }
            }
            Row {
                anchors.right: parent.right
                spacing: 6
                Repeater {
                    model: panel.swatches
                    delegate: Rectangle {
                        id: sw
                        required property var modelData
                        readonly property bool picked: QbzLyrics.useCustomColor
                            && Qt.colorEqual(QbzLyrics.customColor, sw.modelData)
                        width: 18
                        height: 18
                        radius: 9
                        color: sw.modelData
                        border.width: sw.picked ? 2 : (swArea.containsMouse ? 1 : 0)
                        border.color: sw.picked ? theme.textPrimary : theme.borderMuted
                        MouseArea {
                            id: swArea
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                QbzLyrics.useCustomColor = true
                                QbzLyrics.customColor = sw.modelData
                                QbzLyrics.prefsChanged()
                            }
                        }
                    }
                }
            }
        }

        // --- Uppercase -----------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 22
            Text {
                anchors.left: parent.left
                anchors.right: upToggle.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Uppercase", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzToggle {
                id: upToggle
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                checked: QbzLyrics.uppercase
                onToggled: function (value) {
                    QbzLyrics.uppercase = value
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Lite fill (perf) -----------------------------------------------
        Item {
            width: panel.rowWidth
            height: 22
            Text {
                anchors.left: parent.left
                anchors.right: liteToggle.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Lite fill (saves CPU)", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzToggle {
                id: liteToggle
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                checked: QbzLyrics.liteFill
                onToggled: function (value) {
                    QbzLyrics.liteFill = value
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Dimming --------------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 34
            Text {
                anchors.left: parent.left
                anchors.right: dimSelect.left
                anchors.rightMargin: 8
                height: parent.height
                text: QbzSession.tr("Dimming", QbzSession.trRev)
                color: theme.textSecondary
                font.pixelSize: 12
                verticalAlignment: Text.AlignVCenter
                elide: Text.ElideRight
            }
            QbzSelect {
                id: dimSelect
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                menuWidth: 120
                options: [
                    QbzSession.tr("Off", QbzSession.trRev),
                    QbzSession.tr("Soft", QbzSession.trRev),
                    QbzSession.tr("Strong", QbzSession.trRev)
                ]
                currentIndex: QbzLyrics.dimmingMode
                onSelected: function (i) {
                    QbzLyrics.dimmingMode = i
                    QbzLyrics.prefsChanged()
                }
            }
        }

        // --- Footer ----------------------------------------------------------
        Item {
            width: panel.rowWidth
            height: 34
            Row {
                anchors.right: parent.right
                spacing: 8
                SettingsButton {
                    width: 112
                    text: QbzSession.tr("Copy lyrics", QbzSession.trRev)
                    enabled: panel.canCopy
                    onClicked: panel.copyLyrics()
                }
                SettingsButton {
                    width: 84
                    text: QbzSession.tr("Reset", QbzSession.trRev)
                    onClicked: QbzLyrics.resetPrefs()
                }
            }
        }
    }
}
