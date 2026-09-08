// The Qobuz-mix gradient square — ONE definition of the four mix identities
// (colours, badge, display name), used by BOTH surfaces that draw them:
//
//   views/HomeView.qml  MixesRail  — 220px tile, badge + 22px name
//   views/MixView.qml   header     — 224px art, no badge, 34px name
//
// In the Slint build those two live in different files and each re-declares
// the same four gradients (discover/QobuzMixesRow.slint:79-104 and
// mix/MixView.slint:63-70). Porting that duplication would mean a colour
// changed on the tile and not on the page it opens, so the port keeps one
// source (track rule 5).
//
// GRADIENT DELTA, inherited from the existing MixesRail: the .slint uses
// `@linear-gradient(135deg, a 0%, b 50%, c 100%)` and QML's Gradient has no
// angle — only Vertical / Horizontal. The vertical 3-stop plus a
// top-left-to-bottom-right darkening overlay is the approximation the POC
// already shipped for the tiles; the page header now uses the same one, so
// the two match each other exactly.

import QtQuick
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    /// "daily" | "weekly" | "fav" | "top".
    property string kind: "daily"
    property int size: 220
    property int titleSize: 22
    property int cornerRadius: 8
    property bool showBadge: true
    /// Whether the square reacts to the pointer at all (the page header does
    /// not — it is not a link to anywhere).
    property bool interactive: true

    signal clicked()

    QbzTheme { id: theme }

    width: root.size
    height: root.size

    // QobuzMixesRow.slint:79-104 — the badge is the SERVICE the mix comes
    // from ("qobuz" = Qobuz's own dynamic mixes, "qbz" = built locally from
    // the user's library / playlists), which is why it is not uniform.
    readonly property var _spec: ({
        "daily": { "badge": "qobuz", "name": "DailyQ",
                   "c0": "#1e3a8a", "c1": "#6366f1", "c2": "#c084fc" },
        "weekly": { "badge": "qobuz", "name": "WeeklyQ",
                    "c0": "#065f46", "c1": "#10b981", "c2": "#fbbf24" },
        "fav": { "badge": "qbz", "name": "FavQ",
                 "c0": "#7f1d1d", "c1": "#ef4444", "c2": "#fb923c" },
        "top": { "badge": "qbz", "name": "TopQ",
                 "c0": "#1f2937", "c1": "#4b5563", "c2": "#fbbf24" }
    })
    readonly property var spec: root._spec[root.kind] !== undefined
        ? root._spec[root.kind] : root._spec["daily"]
    readonly property string mixName: root.spec.name
    readonly property string badge: root.spec.badge

    /// The four tile descriptions (QobuzMixesRow.slint:82-103). NOTE these
    /// are NOT the mix PAGE's subtitles — `mix.rs::mix_meta` uses a different
    /// string for `weekly` and `top`, and the page reads its own from Rust.
    function tileDescription(k) {
        return k === "daily"
            ? QbzSession.tr("Elevate your day with a customized selection of music.", QbzSession.trRev)
            : k === "weekly"
            ? QbzSession.tr("Take a weekly journey with a fresh mix every Friday.", QbzSession.trRev)
            : k === "fav"
            ? QbzSession.tr("A fresh shuffle from your personal library.", QbzSession.trRev)
            : QbzSession.tr("Discover new music from your most-played playlists.", QbzSession.trRev)
    }

    Rectangle {
        anchors.fill: parent
        radius: root.cornerRadius
        gradient: Gradient {
            GradientStop { position: 0.0; color: root.spec.c0 }
            GradientStop { position: 0.5; color: root.spec.c1 }
            GradientStop { position: 1.0; color: root.spec.c2 }
        }
        // The 135deg sweep, approximated (see the header note).
        Rectangle {
            anchors.fill: parent
            radius: root.cornerRadius
            gradient: Gradient {
                GradientStop { position: 0.0; color: "#00000000" }
                GradientStop { position: 1.0; color: "#33000000" }
            }
        }
        Text {
            visible: root.showBadge
            x: 12
            y: 12
            text: root.badge
            color: "#ccffffff"
            font.pixelSize: 10
            font.weight: theme.weightSemibold
            font.letterSpacing: 1
        }
        Text {
            anchors.centerIn: parent
            text: root.mixName
            color: "#ffffff"
            font.pixelSize: root.titleSize
            font.weight: theme.weightBold
        }
        MouseArea {
            anchors.fill: parent
            enabled: root.interactive
            cursorShape: Qt.PointingHandCursor
            onClicked: root.clicked()
        }
    }
}
