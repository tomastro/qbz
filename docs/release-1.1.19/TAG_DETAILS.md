# 1.1.19 — Limpiando la casa

Massive housekeeping release with two headline features: **Scene Discovery** for exploring artists by location and musical scene, and a **redesigned Home with 3 Discovery Tabs** (Home, Editor's Picks, For You). Also upgraded the audio engine, hardened security, rebuilt the booklet viewer, and added three new Neon visualizers — all while fixing dozens of stability issues across shutdown, window management, and audio routing.

---

## Scene Discovery

Discover artists from the same city, country, or musical scene as any artist in your library. Powered by MusicBrainz location and genre affinity matching, validated against the Qobuz catalog.

  - **Two view modes** — responsive grid or sidepanel with selected artist's discography
  - **A-Z grouping** — alphabetical jump navigation for large scenes
  - **Genre multi-select filter** — searchable popup when >9 genres; real-time filtering with count display
  - **Text search** — filter discovered artists by name
  - **Sticky compact header** — scene label, country flag, and genre summary appear on scroll
  - **Progressive loading** — animated phases (searching → validating → done) with paginated "Load More"

## Discovery Tabs — Redesigned Home

The Home view has been redesigned as a 3-tab discovery experience:

  - **Home** — customizable feed with drag-to-reorder sections and per-section visibility toggles. Sections include New Releases, Press Awards, Popular Albums, Qobuzissimes, Editor's Picks, Qobuz Playlists, Your Mixes, Essential Discography, Recently Played, Continue Listening, Your Top Artists, and More From Favorites
  - **Editor's Picks** — fixed Qobuz editorial curation in a curated order
  - **For You** — personalized recommendations built from your listening history:
    - **Radio Stations** — 9 album-based radios with dominant color extraction per card
    - **Artists to Follow** — 10 suggestions from similar-to-your-top-3 artists
    - **Spotlight** — featured artist with top tracks, playlists, and "Create Radio" button
    - **Similar to [Album]**, **Rediscover Your Library**, and **Essentials [Genre]** sections
  - **Sticky compact header** — appears after 60px scroll with tab navigation and genre filter
  - **Session cache** — cached data and per-tab scroll position for instant back-navigation
  - **Time-based greeting** — Morning/Afternoon/Evening with username, can be disabled

---

## Audio Engine Upgrade — rodio 0.22

The vendored rodio/cpal fork has been removed entirely. QBZ now uses upstream rodio 0.22, which means faster updates, fewer maintenance headaches, and broader hardware compatibility.

  - **ALSA sample rate fallback** — if your DAC doesn't support the requested rate, QBZ now falls back gracefully instead of failing silently
  - **PipeWire suspension** — PipeWire sink is suspended before ALSA Direct takes exclusive access, eliminating DeviceBusy errors on first play
  - **Gapless playback on ALSA Direct** — gapless is now available on all backends and defaults to ON
  - **Smart quality downgrade** — when hardware can't handle a sample rate, QBZ automatically selects the best compatible quality and shows a tooltip explaining why

## Booklet Viewer — MuPDF

  - **PDF booklets** — albums with digital booklets now show a booklet button; opens a full in-app viewer
  - **MuPDF backend** — replaced pdfjs-dist with native MuPDF rendering for crisp, fast page display
  - **Download button** — save the booklet PDF directly from the viewer

## Neon Visualizers

Three new Canvas 2D visualizers under the Neon category in Immersive mode:

  - **Laser** — neon laser beams that pulse and bend with the music
  - **Tunnel** — depth-tunnel effect with warped rings and flowing wisps
  - **Comet** — watercolor nebula with curved wisps, tunnel rings, and a central abyss; extracts colors from album artwork so each album has its own palette

All three respond dynamically to bass, mids, and highs. The Comet visualizer fades nearly to black during silence and breathes back to life when the music returns. Subtle corner vignettes using the darkest album art color protect track info text readability.

## Immersive Improvements

  - **3-mode background** — Full (GPU shader), Lite (CSS blur), or Off; auto-degrades on sustained low FPS with a dismissible notification
  - **Per-panel FPS** — each visualizer panel has its own FPS setting; defaults to 15 FPS for battery life
  - **Configurable ambient FPS** — separate from panel FPS

## Artist Discovery — MusicBrainz Tags

  - **Tag-based recommendations** — "You May Also Like" in the Artist Network sidebar now uses MusicBrainz genre tags to find related artists, breaking away from the typical "similar artist" bubble
  - **Tag-scoped thumbs down** — dismiss an artist from a specific genre context (e.g., AC/DC dismissed from "electronic" but still valid in "metal"); instant removal with reserve replacement
  - **Similarity percentage** — each recommended artist shows how closely they match based on shared tags

## Artist Network Sidebar

  - **Open by default** — sidebar auto-opens when visiting an artist page; preference persisted
  - **State restoration** — closing the sidebar for Queue or Lyrics remembers the previous state and restores it when they close
  - **Clean transitions** — no more stale data flash when navigating between artists
  - **Simplified header** — "Artist Network" title with PanelRightClose icon; empty relationship sections are hidden

## Security Hardening

  - Removed hardcoded key material from credential encryption
  - Cryptographically secure session ID generation
  - DOM-based HTML sanitization replacing regex
  - Sensitive identifiers redacted from backend logs
  - Hardened CI workflow permissions and input handling

## Label Releases View

  - **Redesigned** — new layout with label logo, sorting (date/title/popularity), filters (album/EP/single/live), and search
  - **Group by artist** — toggle to organize releases by artist with collapsible sections
  - **Performance** — virtualized rendering for labels with hundreds of releases

## New Features

  - **Scene Discovery** — explore artists by location and musical scene (see above)
  - **Discovery Tabs** — redesigned Home with 3-tab layout (see above)
  - **Explicit content badges** — shown across the entire app where applicable
  - **Search track context menu** — right-click on search result tracks now works
  - **Track/disc number extraction** — local library now reads track and disc numbers from file tags during import
  - **Spotify import via embed scraping** — removed API dependency; imports work without Spotify credentials
  - **Window size validation** — prevents GDK crashes from corrupt stored dimensions (by @arminfelder, PR #136)
  - **GPU detection** — AMD and Intel GPU identification for composition profile recommendations
  - **GSK renderer control** — choose between Cairo, GL, NGL, or Vulkan rendering backends
  - **Cancel download** — purchase album downloads can now be cancelled mid-progress
  - **Miniplayer shortcuts** — keyboard shortcuts for miniplayer controls

## Stability Fixes

  - **Graceful shutdown** — eliminated heap corruption on exit caused by double-destroy of WebKit windows
  - **Window size clamping** — prevents oversized windows from crashing GDK on restore
  - **Secondary window teardown** — miniplayer and secondary windows close cleanly before app exit
  - **Flatpak tray icon** — correct icon path resolution inside the Flatpak sandbox
  - **Snap MPRIS** — declared as slot instead of plug for proper media control integration

## Bug Fixes

  - Equalizer animation no longer appears active when player is paused
  - Home settings changes apply immediately without page reload
  - Navigation scroll position is now per-item with 1-hour TTL
  - Session restore no longer recalls stale scroll positions
  - Download paths restructured to Artist/Album [FORMAT][Quality]/track
  - EPs and singles backfilled when artist page omits them
  - Audio settings sync on startup for OAuth login path
  - Output device name updates during playback via polling
  - Svelte a11y and component warnings resolved across 26+ components

---

Special thanks to **@arminfelder** for contributing window size validation (PR #136).

Full changelog: https://github.com/vicrodh/qbz/compare/v1.1.18...v1.1.19
