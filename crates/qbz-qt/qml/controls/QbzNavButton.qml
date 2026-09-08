// QbzNavButton — the 28px circular page/history chevron (Slint NavButton),
// consolidated in phase 22 from THREE identical copies (HomeView.NavBtn,
// SectionRail.RailNavBtn, HeaderBar.HeaderNavBtn). Contract: icon `name`,
// `btnEnabled` (dims to 0.4 + muted glyph, disarms), `clicked()`.
//
// Geometry and states are 1:1 with BOTH Slint originals: 28x28, r14,
// surface-elevated idle / surface-hover on hover, opacity 0.4 disabled, glyph
// 15x15 at a WHOLE-PIXEL offset (28-15 = 13 -> centerIn would land on 6.5px
// and render blurry; Slint rounds deliberately, HeaderBar.slint:329-330).
//
// The IDLE FILL is where the two originals part company, and the difference
// is not cosmetic drift:
//   - HeaderNavBtn (shell/HeaderBar.slint:312-342) has two arms, hover and
//     idle. Its host — the header chrome bar — is ALREADY the translucent
//     surface under the ambient background (HeaderBar.qml: surfaceCardA50),
//     so the disc stays opaque and reads as a solid control on frosted
//     chrome.
//   - The six carousel NavButtons (discover/Carousel.slint:15-45, and the
//     byte-identical copies in ArtistCarousel / PlaylistCarousel /
//     RadioCarousel / SlimCarousel / PinnedCarousel) have a THIRD arm: with
//     `ShellState.app-background-active` they idle at
//     `Theme.surface-elevated.with-alpha(AppearanceState
//     .app-background-surface-alpha)`, so the artwork wash reads THROUGH the
//     chevrons instead of leaving opaque discs pasted on it. They sit on the
//     frosted content panel, not on chrome.
// This file carries that arm behind `ambient` (see below), defaulting OFF —
// which is the header's behaviour, i.e. the shared default is the SAFE one.
//
// WHERE THIS COMPONENT IS MOUNTED — all of it, because the previous version
// of this header named only the two carousel sites and read as if they were
// the whole story. Seven files, thirteen instances, and the ambient decision
// is per HOST SURFACE, not per file:
//
//   CONTENT PANEL (translucent under the wash) — arm ON:
//     controls/QbzSectionHeader.qml:76,82  the carousel page chevrons.
//       WIRED. Those two are the six Slint carousel navigators (that header
//       is a POC consolidation of Home / SectionRail / Search / Label
//       carousel headers, and upstream all four import the carousel
//       components), so `ambient: true` there is 1:1, not a deviation.
//
//   CHROME (opaque control on the frosted chrome bar) — arm OFF:
//     shell/HeaderBar.qml:230,235,241  panel-left + back + forward.
//       Host is HeaderBar's own root, `ambientOn ? surfaceCardA50 :
//       surfaceCard` (HeaderBar.qml:40) — the bar is what goes translucent,
//       the discs on it stay solid. Exactly HeaderNavBtn.
//
//   CONTENT PANEL BY HOST, ARM DELIBERATELY OFF — upstream contradicts
//   itself and the divergence is not this round's to take:
//     views/local/LocalChrome.qml:43,53   settings gear + Plex re-sync
//     views/local/LocalToolbar.qml:187,201  the two square-check-big
//                                           multi-select toggles
//       All four render inside AppShell's contentFrame, which IS the frosted
//       panel (AppShell.qml:149, surface-main @0.22 + hairline), and their
//       Slint root goes fully transparent under the wash
//       (locallibrary/LocalLibraryView.slint:705) so that panel shows
//       through. By host surface these are content panel, same as the
//       carousels. But their upstream counterpart is `CircleIconBtn`
//       (LocalLibraryView.slint:313-321, used at :731 :742 :915 :931) and it
//       has only TWO arms — `ta.has-hover ? surface-hover : surface-elevated`
//       — i.e. Slint leaves opaque discs on its own translucent panel here
//       while giving the carousels the third arm on the same kind of
//       surface. Wiring these four is a one-word edit and probably the
//       correct look; it is also an unapproved app-wide visual divergence,
//       so it stops here with the measurement attached. Owner: `ambient:
//       true` on those four lines is the whole change.
//     views/LocalAlbumView.qml:164,169  the in-page back/forward pair.
//       Content panel by host, and there is NO upstream to be 1:1 with:
//       album/LocalAlbumView.slint:205-208 puts `NavButtons { }` there, and
//       shell/NavButtons.slint is a zero-size placeholder since the real
//       back/forward moved into the header. Slint draws nothing at all at
//       this spot. Left OFF to match the identical HeaderBar pair it
//       duplicates rather than the carousels it sits beside — flag, not
//       conclusion.
//
// The glyph tint is `text-primary` / `text-muted`, NOT a fixed colour: the
// disc is a THEME surface, so on the 11 light themes a literal #ffffff glyph
// is 1.13-1.38:1 against it (APCA Lc 0.0-12.3 — no contrast at all). That was
// the owner-reported bug: this file asked for "primary", which in this port is
// the LEGACY ALIAS OF LITERAL WHITE and not Theme.text-primary (see
// theme/QbzIcon.qml and src/icon_tint_qt.rs `token_for`).

import QtQuick
import com.blitzfc.qbz
import "../theme"

Rectangle {
    property string name: ""
    property bool btnEnabled: true
    /// Carousel arm (Carousel.slint:25-27): let the ambient wash through the
    /// idle disc. OFF by default because the header must NOT do this.
    property bool ambient: false
    signal clicked()

    QbzTheme { id: theme }

    // `ShellState.app-background-active` (state.slint:4184) — the port spells
    // it this way app-wide, npHasTrack included: with nothing playing the
    // shell restores its opaque theme surfaces (rule D4).
    readonly property bool ambientOn: ambient && theme.ambientOn

    width: 28
    height: 28
    radius: 14
    opacity: btnEnabled ? 1.0 : 0.4
    // surface-elevated, at app-background-surface-alpha when the wash is up.
    // `with-alpha` on an opaque token is a plain alpha set, and
    // QbzShell.ambientSurfaceAlpha IS AppearanceState.app-background-surface
    // -alpha — same QBZ_BG_SURFACE_ALPHA knob, same 0.5 default
    // (settings_qt.rs:211 / state.slint:3374).
    color: (nbArea.containsMouse && btnEnabled)
        ? theme.surfaceHover
        : (ambientOn
            ? Qt.rgba(theme.surfaceElevated.r, theme.surfaceElevated.g,
                      theme.surfaceElevated.b, QbzShell.ambientSurfaceAlpha)
            : theme.surfaceElevated)
    QbzIcon {
        name: parent.name
        width: 15
        height: 15
        // Whole-pixel centring (Slint rounds this explicitly; 28-15 = 13, so
        // `anchors.centerIn` would sit at 6.5px and blur the glyph).
        x: Math.round((parent.width - width) / 2)
        y: Math.round((parent.height - height) / 2)
        tintName: parent.btnEnabled ? "textPrimary" : "muted"
    }
    MouseArea {
        id: nbArea
        anchors.fill: parent
        enabled: parent.btnEnabled
        hoverEnabled: true
        cursorShape: parent.btnEnabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: parent.clicked()
    }
}
