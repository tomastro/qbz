# 1.2.7 — Accolade Watch (diagnostics + carousel fix)

Defensive fix for the click-through regression on Discover carousels introduced in 1.2.6, plus a substantial expansion of System Diagnostics (apologies for stacking three releases where one would have been enough).

---

# 1.2.6 — Accolade Watch (accessibility patch)

A quick follow-up on the heels of 1.2.5. Two things drove this release:

- A long-overdue **accessibility pass**: four new WCAG-compliant themes (including a true high-contrast mode for severely vision-impaired users) and mouse-drag scrolling on every horizontal album row.
- A **regression rescue**: Infinite Play (the ∞ toggle in the queue) had silently lost its backend wiring during the V2 migration — the toggle persisted in localStorage but the queue never refilled and the mode was never sent to the backend. Reimplemented from scratch in V2 with no legacy wrappers.

Everything from 1.2.5 below is included as well; if you're upgrading from 1.2.4 or earlier, this is what you're getting.

---

## What's new in 1.2.6

### Accessibility (discussion #313)

  - **Four new WCAG-compliant themes** in Settings > Appearance > Theme:
    - **WCAG Light** — AAA contrast (7:1+) on every text level, blue accent
    - **WCAG Dark** — AAA contrast on dark backgrounds
    - **High Contrast** — pure white on pure black with yellow accent (21:1 ratio). Built explicitly for severely vision-impaired users
    - **Colorblind** — deuteranopia-safe palette (Wong colors), no red/green adjacency
  - **Mouse-drag scrolling on horizontal album rows** — click and drag sideways on Home, For You, Award and Label views, just like on iPad or in a browser. Arrow buttons still work as before
  - **Qobuz Connect badge always visible** (#318) — even with an empty queue (e.g. on app start) you can connect to a remote renderer instead of having to start playing something first; the badge keeps its size across responsive breakpoints

### Regression fixes

  - **Infinite Play (∞) restored** (#319) — the toggle now persists across restarts and refills the queue with related tracks (Qobuz `/radio/artist`, seeded from your last played tracks) when the queue ends, instead of stopping. V2 reimplementation
  - **Gentoo `qbz-bin` ebuild** — now correctly inherits `xdg-utils`, so the hicolor icon cache and the desktop database are refreshed on install/uninstall. This was the cause of stale icons after upgrading to 1.2.5

### Branding polish

  - **App icon resized to fill the canvas** (matches Steam/Chrome/Papirus footprint, ~97%) with a subtle blue rim and inner highlight so the matte-vinyl disc doesn't disappear on dark backgrounds; new 512x512 PNG export added
  - **Tray icons regenerated** to match the new vinyl Q (the v1.2.5 refresh missed the monochromatic tray glyphs and the Snap launcher)
  - **Login screen** — drop-shadow under the logo removed (the new icon's edge rim and inner light make it redundant)

---

## What 1.2.5 brought (still applies)

After a rough week of real-world work, this release brought a dedicated Award view, labels you can follow, and one of the most anticipated sections from the mobile apps landing on the desktop at last: **Release Watch**.

On the functionality side, you can drag and drop any track — or a multi-selection — from any listing onto the sidebar playlists. The rest of the changes are mostly architectural and not user-facing, but they show up as overall stability.

Regarding Qobuz Connect: I know there's still a lot of room for improvement. Most of it is WIP and will start landing in the coming releases.

I want to mention again that the project has been receiving donations. I had linked the accounts to an email I don't check very often and only noticed last week — I'm overwhelmed (in the good way) and genuinely flattered. I haven't fully decided how to use the funds yet, but part will definitely go toward infrastructure. When I started the project, the free tiers seemed more than enough; now we have hundreds of installations on Snapcraft, thousands on Flathub, and a steady stream of direct downloads from GitHub releases. Since qbz has zero telemetry, I can only keep a partial count — it's not a number I ever imagined reaching. These donations will let me make sure the Cloudflare workers that power the MusicBrainz and Last.fm integrations stay up even if we start to brush against the free-tier limits. I couldn't be more grateful to every sponsor, donor, and everyone who supports the project through code, feedback, or simply by using the app.

Saludos a todos y muchas gracias.

### Accolades (Awards + Press)

  - **Dedicated Award page** — new `AwardView` with header matching the Label/Album conventions, gold press ribbon, and an "Other awards" carousel; wired to V2 commands for `/award/page` and `/award/getAlbums`
  - **AwardAlbumsView** — See-all grid for every award, follow-award as a first-class entity
  - **Award resolution by name** — when `/album/get` omits the award id, the backend now resolves it via `/award/explore`, normalises ids end-to-end as strings, and tolerates inconsistent Qobuz shapes on `AwardPageData` / `AwardMagazine`
  - **Editorial seed** — `/award/explore` seeds across all locales with diacritic-insensitive lookup; hard-coded seed removed
  - **Album-view right sidebar** — shows the full awards stack plus the album's label
  - **Album ribbons extended to press accolades** — last award wins on the card; the dedicated sidebar still shows the full stack
  - **Home cleanup** — editorial ribbons added for Qobuzissime and Album of the Week on the home rails

### Release Watch

  - **Dedicated tabbed view** — mirrors the Qobuz mobile layout with its own entry in Discover; moved below Your Mixes in the home grid
  - **Backend** — `/favorite/getNewReleases?type=artists` as REST (not signed RPC), projected onto `SearchResultsPage`, artist field backfilled from `artists[0]`
  - **Persistence** — Release Watch now survives home-cache invalidations so it renders immediately on revisit

### Labels

  - **Follow / unfollow labels** — mirrors Follow Artist; heart overlay on `LabelView`'s more-labels cards
  - **Favorites → Labels tab** — new tab in the Favorites view

### Visual identity

  - **New matte-vinyl logo** applied to every icon asset: Tauri standard sizes, Windows Store squares, Android icons and mipmaps, `static/`, README, tray
  - **Monochrome symbolic variants** — `tray-light.png` (black glyph) and `tray-dark.png` (white glyph) shipped alongside `tray.png`; the Linux tray picks the matching variant at runtime by probing GNOME `color-scheme`, GTK `prefer-dark-theme`, and KDE `kdeglobals`
  - **Login screen refresh** — logo resized to 175px, plus QBZ and "Qobuz™ Player" branding under the mark; `app.tagline` rewritten in en/es
  - **About dialog** — fixed a duplicated "License" label (the second row was meant to show the platform); codename updated to "Accolade Watch"
  - **DAC Setup Wizard icon contrast** — button icon drops `invert(1)` so it reads on light accent themes (Catppuccin Lavender, Dracula…); the tooltip copy of the same icon now has `invert(1)` so it reads on the dark surface bg

### Desktop integration (Linux)

  - **Window controls auto-detect** — new `desktop_theme` Rust module reads `kwinrc`, `kdeglobals`, and `klassyrc`; a runtime-derived "Klassy (auto-detect)" or "Plasma (auto-detect)" preset mirrors the system decoration colors and button shape when Plasma is detected, and stays hidden otherwise
  - **Klassy button shapes** — `mapKlassyShapeToQbz()` maps Klassy's `IntegratedRoundedRectangle`, `FullHeightRectangle`, `FullHeightRoundedRectangle`, `Tab`, `Circle`, `Square` onto QBZ's `ButtonShape` set; adds a new `full-height-rounded` variant to cover Klassy's most common preset
  - **Opt-in rounded window corners** — "Match system window chrome" toggle persists to `window_settings.db` and gates the Tauri window transparency decision at startup; on next launch the main window is rebuilt transparent and the detected corner radius (10 Klassy, 6 Breeze, 12 Adwaita, 8 fallback) is applied via `clip-path` + `border-radius` with GPU compositing for clean anti-aliasing on WebKitGTK
  - **ksni tray on Linux** — replaces the libayatana-appindicator path so left-click actually toggles the window (issue #310)
  - **Window size clamp** — persisted sizes that exceed the largest available monitor now clamp to fit instead of opening off-screen

### Streaming, audio, and player

  - **Request signing expanded** — all remaining Qobuz API endpoints and every search endpoint now sign their requests; stronger compliance and fewer 403 edge cases
  - **BitPerfectMode surfaced in QualityBadge** (#288) — mode now flows through `PlaybackEvent` and the badge updates on playback start
  - **DAC rate unsupported fallback** (#288) — track quality is downgraded and the backend falls back to `plughw` instead of CPAL when the DAC can't handle a requested rate; clearer error when CPAL cannot open an enumerated device
  - **Output device missing notification** (#307) — user gets a toast when the selected output device disappears
  - **Audio output badges refresh** on playback start
  - **ALSA Direct `hw` immersive volume lock** — volume slider is disabled in the immersive player when ALSA Direct hw is active (bit-perfect lock)
  - **ALSA default plugin** — `alsa_plugin` defaults to `Hw` when switching to the ALSA backend
  - **Gapless loop-one fix** — logic moved from backend to frontend; loop-one now works with gapless playback
  - **Gapless local-library fallback** — tries local library if not in any cache; fails silently when track isn't found
  - **ALSA engine shutdown** — engine stops before dropping the ALSA stream on format changes
  - **Real offline mode** (#279) — snapshot streaming, network blocked, diagnostic logs
  - **Player fixes** — `durationSecs` passed on session-restore first play so the seekbar advances (previously stuck); qconnect always persists local session to preserve track-level restore (#304)

### Updater (opt-in, non-store builds)

  - **tauri-plugin-updater integrated** — backend + frontend; `UpdateProgressModal` added with i18n; wired into app bootstrap and Settings
  - **Gated behind Cargo feature `updater`** — sandboxed builds (Flathub, Snap) pass `--no-default-features` so the updater stays disabled for store-managed installs
  - **Signed manifest** — minisign public key baked in; CI signs artifacts and consolidates the update manifest; updater manifest path no longer triggers an infinite `tauri dev` rebuild loop

### UI fixes and polish

  - **Smart positioning for sidebar menus** — the per-playlist right-click menu now flips above the cursor when it would fall below the window; the Sort submenu inside the general playlists menu uses `use:portal` to escape clipping by `.dropdown-menu`
  - **Genre filter popup** — collision check reads the CSS `max-height` (530/630 width, 500/700 height) instead of the measured rect so the popup flips above the trigger or clamps to the viewport when content grows asynchronously
  - **Right-section collapse** (#303) — hamburger at narrow widths instead of overflow
  - **QualityBadge / QconnectBadge compact variants** — keeps them at full bar height without overflow
  - **Silk animations removed** from ForYouTab mix cards and Your Mixes covers — performance cost outweighed the visual benefit and they caused minor rendering glitches
  - **Multi-select drag** enabled in all views; track drag & drop extended to artist and search views and onto sidebar playlists; compact drag ghost with artist + album
  - **Lyrics active line** uses theme accent color
  - **For-You load order** — waits for `topArtists`/`recentAlbums` before loading dependent rails
  - **Banner layout** uses a CSS variable; i18n key fixed; responsive height
  - **Favorites Select All** — new checkbox in multi-select mode across track views

### Internal architecture

  - **`commands_v2` split into a module directory** — the former single-file module is now nineteen focused files, easier to navigate and review
  - **Custom device name** persists across restarts for QConnect
  - **Prefetch perf** — cache depth increased to 5 tracks with 2 concurrent CMAF segment downloads in parallel
  - **Dependency bumps** — tauri-plugin-dialog 2.7.0, tauri-plugin-deep-link, rodio 0.22.2, vite 8.0.8, svelte 5.55.2, sveltejs/kit 2.57.1, rand, jsdom 29.0.2, notify-rust 4.14.0

### Packaging

  - **Snap** — MPRIS slot restored after snapcraft approval; exclusive-audio note added to the description
  - **CI ARM64** — native ARM runner replaces QEMU for the aarch64 build; targets glibc 2.35; dropped OpenSSL in favour of rustls across all crates

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.5...v1.2.6
