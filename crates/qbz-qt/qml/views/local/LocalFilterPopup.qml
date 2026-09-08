// Albums quality/format/source filter popup (LocalLibraryView.slint:2470 —
// the component at the end of the file). Floats OVER the content: a
// click-out backdrop plus a card pinned 36px from the right edge, 92px down,
// radius 10, surface-card, 1px subtle border.
//
// The card's 372px is a FLOOR, not the width. The Slint original sized a fixed
// card around three source chips; this port grew to five (Plex plus the two
// media servers) and the row overflowed the card — owner report, 2026-08-20.
// The width is derived from the widest chip row instead, so it also survives a
// UI-scale bump or a language whose labels are longer, neither of which a fixed
// number can. `implicitWidth` is safe to measure here: a `Row` sums its
// children and a non-wrapping `Text` reports its natural width, so nothing in
// the chain reads the card width back and there is no binding loop.
//
// Three sections of FilterChips: Quality (Hi-Res / CD / Lossy), Format
// (FLAC ALAC APE WAV, then MP3 AAC Other), Source (Local / Offline cache /
// Plex). "Clear" appears in the title row only when something is active.
//
// The Slint card carries a 28px drop shadow; QML shadows need the graphical
// effects path, which renders nothing on this port's software renderer
// (QbzIcon.qml documents the same trade), so the border carries the edge.

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Item {
    id: root

    property var view: null

    QbzTheme { id: theme }

    // Click-out backdrop.
    MouseArea {
        anchors.fill: parent
        onClicked: root.view.filterOpen = false
        // A MouseArea eats presses, NOT wheel events — the grid behind this
        // card kept scrolling with it open.
        onWheel: function (wheel) { wheel.accepted = true }
    }

    Rectangle {
        id: card
        x: parent.width - width - 36
        y: 92
        /// Widest row, plus the 16px padding on each side. Capped so the card
        /// can never run past the left edge of the window.
        readonly property real contentWidth: Math.max(
            titleText.implicitWidth + (root.view.filterCount > 0 ? clearBtn.width + 8 : 0),
            qualityRow.implicitWidth,
            formatRow1.implicitWidth,
            formatRow2.implicitWidth,
            favoriteRow.visible ? favoriteRow.implicitWidth : 0,
            sourceRow.implicitWidth)
        width: Math.min(Math.max(372, card.contentWidth + 32),
                        Math.max(372, root.width - 72))
        height: col.height + 32
        color: theme.surfaceCard
        radius: 10
        border.width: 1
        border.color: theme.borderSubtle

        // Swallow clicks so the backdrop does not close the card, and the
        // wheel so it does not reach the grid.
        MouseArea {
            anchors.fill: parent
            onWheel: function (wheel) { wheel.accepted = true }
        }

        Column {
            id: col
            x: 16
            y: 16
            width: parent.width - 32
            spacing: 10

            Row {
                width: parent.width
                height: 28
                Text {
                    id: titleText
                    width: parent.width - (root.view.filterCount > 0 ? clearBtn.width : 0)
                    height: parent.height
                    text: QbzSession.tr("Filter", QbzSession.trRev)
                    color: theme.textPrimary
                    font.pixelSize: theme.fontHeading
                    font.weight: theme.weightSemibold
                    verticalAlignment: Text.AlignVCenter
                }
                Rectangle {
                    id: clearBtn
                    visible: root.view.filterCount > 0
                    width: clearText.implicitWidth + 18
                    height: 28
                    radius: 6
                    color: clearArea.containsMouse ? theme.surfaceHover : "transparent"
                    Text {
                        id: clearText
                        anchors.centerIn: parent
                        text: QbzSession.tr("Clear", QbzSession.trRev)
                        color: theme.accent
                        font.pixelSize: theme.fontLegal
                        font.weight: theme.weightMedium
                    }
                    MouseArea {
                        id: clearArea
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.view.clearFilter()
                    }
                }
            }

            // Album membership lives here, not in the three chained Genres
            // columns: it narrows the resulting album set just like quality,
            // format and source do.
            Text {
                visible: root.view.activeTab === "albums"
                    || root.view.activeTab === "genres"
                text: QbzSession.tr("Favorites", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
            }
            Row {
                id: favoriteRow
                visible: root.view.activeTab === "albums"
                    || root.view.activeTab === "genres"
                spacing: 8
                FilterChip {
                    label: QbzSession.tr("Favorites only", QbzSession.trRev)
                    active: root.view.filter.favorite === true
                    onToggled: root.view.toggleFilter("favorite")
                }
            }

            // ---- Quality ----
            Text {
                text: QbzSession.tr("Quality", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
            }
            Row {
                id: qualityRow
                spacing: 8
                // DSD is a tier of its own (the badge already wears one);
                // Hi-Res is 24-bit PCM only. Proper name, not translated.
                FilterChip {
                    label: "DSD"
                    active: root.view.filter.dsd === true
                    onToggled: root.view.toggleFilter("dsd")
                }
                FilterChip {
                    label: QbzSession.tr("Hi-Res", QbzSession.trRev)
                    active: root.view.filter.hires === true
                    onToggled: root.view.toggleFilter("hires")
                }
                FilterChip {
                    label: QbzSession.tr("CD", QbzSession.trRev)
                    active: root.view.filter.cd === true
                    onToggled: root.view.toggleFilter("cd")
                }
                FilterChip {
                    label: QbzSession.tr("Lossy", QbzSession.trRev)
                    active: root.view.filter.lossy === true
                    onToggled: root.view.toggleFilter("lossy")
                }
            }

            // ---- Format ----
            Text {
                text: QbzSession.tr("Format", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
            }
            Row {
                id: formatRow1
                spacing: 8
                FilterChip { label: "FLAC"; active: root.view.filter.flac === true; onToggled: root.view.toggleFilter("flac") }
                FilterChip { label: "ALAC"; active: root.view.filter.alac === true; onToggled: root.view.toggleFilter("alac") }
                FilterChip { label: "APE"; active: root.view.filter.ape === true; onToggled: root.view.toggleFilter("ape") }
                FilterChip { label: "WAV"; active: root.view.filter.wav === true; onToggled: root.view.toggleFilter("wav") }
            }
            Row {
                id: formatRow2
                spacing: 8
                FilterChip { label: "MP3"; active: root.view.filter.mp3 === true; onToggled: root.view.toggleFilter("mp3") }
                FilterChip { label: "AAC"; active: root.view.filter.aac === true; onToggled: root.view.toggleFilter("aac") }
                FilterChip {
                    label: QbzSession.tr("Other", QbzSession.trRev)
                    active: root.view.filter.other === true
                    onToggled: root.view.toggleFilter("other")
                }
            }

            // ---- Source ----
            Text {
                text: QbzSession.tr("Source", QbzSession.trRev)
                color: theme.textMuted
                font.pixelSize: theme.fontLegal
                font.weight: theme.weightSemibold
            }
            Row {
                id: sourceRow
                spacing: 8
                FilterChip {
                    label: QbzSession.tr("Local", QbzSession.trRev)
                    active: root.view.filter.local === true
                    onToggled: root.view.toggleFilter("local")
                }
                FilterChip {
                    label: QbzSession.tr("Offline cache", QbzSession.trRev)
                    active: root.view.filter.offline === true
                    onToggled: root.view.toggleFilter("offline")
                }
                FilterChip {
                    visible: QbzLocal.plexAvailable
                    label: "Plex"
                    active: root.view.filter.plex === true
                    onToggled: root.view.toggleFilter("plex")
                }
                // The media servers, chips of their own. They are only worth
                // a chip when the user actually has one — an always-present
                // "Jellyfin" that can never match anything is a control that
                // teaches the user the filter is broken.
                FilterChip {
                    visible: QbzLocal.mediaHasJellyfin
                    label: "Jellyfin"
                    active: root.view.filter.jellyfin === true
                    onToggled: root.view.toggleFilter("jellyfin")
                }
                FilterChip {
                    visible: QbzLocal.mediaHasSubsonic
                    label: "Subsonic"
                    active: root.view.filter.subsonic === true
                    onToggled: root.view.toggleFilter("subsonic")
                }
            }
        }
    }
}
