// ArtistScene › the A-Z jump rail — 1:1 with Tauri's `.alpha-index`
// (ArtistsByLocationView.svelte markup :699-711, CSS :1372-1413), per
// 01-tauri-artist-scene.md §11.
//
// NOT controls/QbzAlphaStrip.qml. That one is the Library/Local rail: an 18px
// gutter of 15px rows in 9px muted type, driven by a { letter, index } list of
// the jumps that EXIST. This one is Tauri's: a fixed 27-slot pill — `#` first,
// then A-Z, every slot always drawn — 20×20 letters at 11/600 on a translucent
// black card, where a letter with no members stays visible at opacity 0.2.
// Different alphabet contract, different metrics, different host; the
// reference has two components as well.
//
// STATIC, NOT STICKY. §11 and §18 of the spec both call this rail "sticky"
// (`position:sticky; top:0`), and 08-critique-findings settles that the sticky
// never engages: `.alpha-index` is a flex child of a row that does not scroll,
// and the real scroller is a descendant of its SIBLING. What pins it to the
// top is `align-self: flex-start`. So: the host anchors this item's TOP and
// lets it keep its implicit height. Do not wire it to a Flickable's contentY.
//
// DELIBERATE DIVERGENCE (recorded, small): Tauri styles a memberless letter
// `.disabled` — opacity 0.2, `cursor: default` — but leaves the <button> live
// and early-returns inside `scrollToGroup` (V:310). Here the letter is
// genuinely inert: no hover paint, no pointer cursor, no signal. Same
// user-visible result with less surprise, and one fewer no-op code path.
//
// MOUNTING — the root IS the 28px pill, and `.alpha-index`'s 4px `margin-left`
// is the HOST's (ArtistSceneView.qml:1123 already writes it as
// `x: gridPane.width + 4`). Do not bake it in here as well.
//
//     AlphaIndexRail {
//         x: gridPane.width + 4             // the CSS margin-left
//         y: 0                              // align-self: flex-start
//         activeKeys: root.alphaActiveKeys  // ["#","A","D",…] or {"A": true}
//         onJumped: function (letter) { … } // host scrolls its own view
//     }

import QtQuick
import com.blitzfc.qbz
import "../../theme"

Item {
    id: root

    // --- the frozen mount API (00-CONTRACT.md, scene lane) ---------------
    /// Which of the 27 slots have members. Accepts an array (["#","A","D"])
    /// or a set-shaped object ({"A": true}) — the host may hold either.
    property var activeKeys: []
    signal jumped(string letter)

    QbzTheme { id: theme }

    // `ALPHA_LETTERS = '#ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('')` (V:271) —
    // 27 entries, `#` FIRST. Built once, never re-derived: this is a constant.
    readonly property var letters: [
        "#", "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
        "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z"
    ]

    function hasMembers(letter) {
        var k = root.activeKeys
        if (!k)
            return false
        if (Array.isArray(k))
            return k.indexOf(letter) >= 0
        return k[letter] === true
    }

    // 20px letters inside 4px of side padding — `padding: 6px 4px`.
    implicitWidth: 28
    implicitHeight: pill.height
    width: implicitWidth
    height: implicitHeight

    Rectangle {
        id: pill
        width: 28
        height: col.height + 2 * 6
        radius: 10
        // `background: rgba(0,0,0,0.2)`. Qt.rgba with 0..1 floats, NEVER an
        // 8-digit hex — Qt reads #AARRGGBB where CSS writes #RRGGBBAA, so
        // "#00000033" would be a fully transparent black here.
        color: Qt.rgba(0, 0, 0, 0.2)

        Column {
            id: col
            x: 4
            y: 6
            spacing: 2

            Repeater {
                model: root.letters
                delegate: Rectangle {
                    id: slot
                    required property var modelData

                    readonly property bool live: root.hasMembers(slot.modelData)

                    width: 20
                    height: 20
                    radius: 4
                    color: (slot.live && slotArea.containsMouse) ? theme.surfaceElevated
                                                                 : "transparent"
                    // `.disabled { opacity: 0.2 }` / `opacity: 0.9` -> 1 on
                    // hover. `transition: all 100ms ease` — hover-bounded, the
                    // one animation class the pulse law leaves alone.
                    opacity: !slot.live ? 0.2 : (slotArea.containsMouse ? 1.0 : 0.9)
                    Behavior on opacity { NumberAnimation { duration: 100 } }
                    Behavior on color { ColorAnimation { duration: 100 } }

                    Text {
                        anchors.fill: parent
                        text: slot.modelData
                        color: theme.textPrimary
                        font.pixelSize: 11
                        font.weight: theme.weightSemibold
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    MouseArea {
                        id: slotArea
                        anchors.fill: parent
                        // Inert when the letter has no members: an enabled
                        // MouseArea would still eat the press and paint a
                        // cursor for a jump that cannot happen.
                        enabled: slot.live
                        hoverEnabled: slot.live
                        cursorShape: Qt.PointingHandCursor
                        onClicked: root.jumped(String(slot.modelData))
                    }
                }
            }
        }
    }
}
