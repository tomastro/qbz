// Per-source glyph — the QML port of `primitives/SourceGlyph.slint`. THE one
// place this port draws a source mark: the version picker, the MyQBZ
// collection-detail rows, the Discography Builder rows, the Local Library
// album/track rows, the Library ALL list rows and the 24x24 card badges of
// discover/AlbumCard.slint + discover/TrackCard.slint all mount it.
//
// Lives in controls/ (the port's "primitives") since 2026-07-31, because the
// cards need it too and Slint keeps its counterpart in `primitives/`. Nothing
// about it was ever Local-Library-specific.
//
// ── THE LOAD-BEARING RULE (SourceGlyph.slint:4-8) ─────────────────────────
// The full-colour Qobuz and Plex logos render **UNTINTED**; only the
// monochrome local hard-drive glyph is tinted. Slint states the reason out
// loud: routing the logos through a tinting primitive "would turn the logos
// into a flat accent silhouette — the bug this primitive replaces". `QbzIcon`
// floods every pixel with its tint, so the logo arms deliberately do NOT go
// through it — they are plain `Image`s.
//
// ── WHY THIS FILE USED TO LIE ─────────────────────────────────────────────
// The port shipped with neither logo: `qobuz-logo-filled.svg` and
// `plex-logo.svg` were never carried over from the Slint tree, so this
// component substituted `cloud-download` for every Qobuz kind and a tinted
// `hard-drive` for Plex — i.e. it drew "downloaded" and "local file" where the
// design calls for two brand marks. Both assets now live in
// `qml/assets/brand/`, which is where the untinted marks belong (the Discogs /
// Last.fm / MusicBrainz marks are already there for the same reason).
//
// `qobuz_purchase` reuses the Qobuz mark GOLD (#eab308) — same glyph as a
// download, the colour is what says "purchased". That is the ONE arm that
// needs a tint on a multi-colour asset, so it is the one arm that uses an
// effect; on a software renderer it degrades to the untinted mark rather than
// to nothing.
//
// ── JELLYFIN AND SUBSONIC COME IN TWO FLAVOURS ────────────────────────────
// Both ship a COLOUR mark and a MONOCHROME one, and `mono` picks. Cards get
// the colour brand mark like Qobuz and Plex; dense rows get the monochrome
// glyph, tinted with `localTint` exactly like the hard-drive.
//
// The monochrome pair goes through `QbzIcon`, NOT through a plain `Image`.
// That is not a style choice — the source files are BLACK ink, so untinted
// they are invisible on every dark theme (rendered and looked at, 2026-08-20).
// `QbzIcon` recolours per theme at runtime with no shader involved, which is
// also why they had to be added to `icon_tint_qt::MASTERS` with their ink
// normalised to #ffffff: that literal IS the tint pipeline's contract.
//
// The Jellyfin colour asset arrived with a WHITE rounded-rect plate behind the
// mark. It was stripped: every other brand mark here is transparent, and a
// white tile beside them reads as a rendering bug rather than a logo.
//
// HONEST CAVEAT on the Subsonic mark: it is NAVIDROME's logo, and `subsonic`
// is one source covering Navidrome, Gonic, Airsonic, Astiga and Ampache. A
// Gonic user sees a Navidrome mark. `ping.view` reports the server's own
// `type`, which is already persisted as `server_name`, so branching the glyph
// later is cheap — this is a known wrongness, not an unnoticed one.

import QtQuick
import QtQuick.Effects
import com.blitzfc.qbz
import "../theme"

Item {
    id: root

    /// "local" | "qobuz" | "qobuz_download" | "qobuz_purchase" | "offline" |
    /// "plex" | "jellyfin" | "subsonic" (anything else = local).
    ///
    /// `"offline"` is the Qt port's word for the Slint's `qobuz_download`
    /// (src/local_rows.rs:223-229 `badge_source`, which folds `qobuz_download`
    /// AND `qobuz_purchase` into it); plain `"qobuz"` is what the Library ALL
    /// feed (library_qt.rs `FeedItem.source`) and the MyQBZ collection items
    /// carry. `SourceGlyph.slint:23` groups all of them with the downloads.
    property string kind: "local"

    /// Size of the MONOCHROME local glyph — and the default for the two brand
    /// marks, because `album/LocalAlbumView.slint:24-26` draws all three kinds
    /// at a flat 14px (the version picker: the call site that sets nothing).
    property int glyphSize: 14
    /// Plex mark size; -1 = `glyphSize`. `SourceGlyph.slint:31` gives Plex the
    /// same 15px as the local glyph, while `AlbumListRow.slint:319` and both
    /// card badges give it 16px — hence a knob and not a rule.
    property int plexSize: -1
    /// Qobuz mark size; -1 = `glyphSize`. 16 in `SourceGlyph.slint:31` /
    /// `TrackRow.slint:722` / `AlbumListRow.slint:319`, 18 on the card badges,
    /// 22 for `qobuz_download` on `AlbumCard.slint:352` — the wordmark's
    /// proportions need the extra pixels.
    property int qobuzSize: -1
    /// Draw the MONOCHROME variant instead of the colour brand mark.
    ///
    /// Only Jellyfin and Subsonic have one; every other kind ignores it. Rows
    /// set it, cards do not — a dense list of coloured logos fights the text,
    /// which is the same reason the local glyph was never a colour mark.
    property bool mono: false
    /// Tint of the MONOCHROME glyphs — the local hard-drive, and the two
    /// media-server marks when `mono`. "secondary" = LocalAlbumView.slint:36;
    /// "muted" = SourceGlyph.slint:28, whose header calls the
    /// muted-never-accent rule load-bearing; "white" = the card badges
    /// (AlbumCard.slint:347 / TrackCard.slint:190), which sit on a near-black
    /// chip.
    property string localTint: "secondary"

    readonly property bool isQobuz: kind === "qobuz" || kind === "qobuz_download"
        || kind === "qobuz_purchase" || kind === "offline"
    readonly property bool isPlex: kind === "plex"
    readonly property bool isPurchase: kind === "qobuz_purchase"
    readonly property bool isJellyfin: kind === "jellyfin"
    // Every brand spelling of a Subsonic server folds to ONE mark, matching
    // `SourceId::from_word` — a row stamped "navidrome" must not fall through
    // to the local hard-drive.
    readonly property bool isSubsonic: kind === "subsonic" || kind === "navidrome"
        || kind === "gonic" || kind === "airsonic" || kind === "astiga"
    readonly property bool isMedia: root.isJellyfin || root.isSubsonic
    /// The media marks in COLOUR (cards); `mono` sends them to `QbzIcon`.
    readonly property bool mediaColour: root.isMedia && !root.mono

    /// Media marks size with the PLEX knob: every call site that sizes Plex is
    /// sizing "a server's logo", and the two new ones belong in that slot.
    /// Giving them a knob of their own would mean every future call site has to
    /// remember a third one.
    width: (root.isPlex || root.isMedia)
        ? (root.plexSize < 0 ? root.glyphSize : root.plexSize)
        : (root.isQobuz
            ? (root.qobuzSize < 0 ? root.glyphSize : root.qobuzSize)
            : root.glyphSize)
    height: width

    readonly property string brandDir: "qrc:/qt/qml/com/blitzfc/qbz/qml/assets/brand/"

    // --- The brand marks: UNTINTED, by rule -------------------------------
    Image {
        id: mark
        anchors.fill: parent
        visible: (root.isQobuz && !root.isPurchase) || root.isPlex || root.mediaColour
        source: root.isPlex ? root.brandDir + "plex-logo.svg"
              : root.isJellyfin && !root.mono ? root.brandDir + "jellyfin-logo.svg"
              : root.isSubsonic && !root.mono ? root.brandDir + "navidrome-logo.svg"
              : root.brandDir + "qobuz-logo-filled.svg"
        fillMode: Image.PreserveAspectFit   // SourceGlyph's `image-fit: contain`
        // Decode at the size actually drawn: these are a 512x512 and a
        // ~1000x1006 source rendered into 14-22px.
        sourceSize.width: root.width
        sourceSize.height: root.height
        smooth: true
        asynchronous: true
    }

    // --- Purchased: the same mark, gold ------------------------------------
    // The only arm that tints a multi-colour asset, so the only one that needs
    // an effect. `_noShaders` degrades to the plain mark — the source is still
    // legible, it just loses the "purchased" colour, which beats an empty cell.
    readonly property bool _noShaders: GraphicsInfo.api === GraphicsInfo.Software
        || GraphicsInfo.api === GraphicsInfo.Null
    Image {
        id: purchaseMark
        anchors.fill: parent
        visible: root.isPurchase
        source: root.brandDir + "qobuz-logo-filled.svg"
        fillMode: Image.PreserveAspectFit
        sourceSize.width: root.width
        sourceSize.height: root.height
        smooth: true
        asynchronous: true
        layer.enabled: root.isPurchase && !root._noShaders
        layer.effect: MultiEffect {
            colorization: 1.0
            // SourceGlyph.slint:27 — the §8.7 purchase gold.
            colorizationColor: "#eab308"
        }
    }

    // --- Monochrome, tinted: the local hard-drive, and the two media marks
    //     when `mono`. All three go through QbzIcon because all three are
    //     single-ink glyphs whose ink has to follow the theme.
    QbzIcon {
        anchors.fill: parent
        visible: !root.isQobuz && !root.isPlex && !root.mediaColour
        name: root.isJellyfin ? "jellyfin"
            : root.isSubsonic ? "navidrome"
            : "hard-drive"
        tintName: root.localTint
    }
}
