# QBZ 1.1.19 Release Highlights

## Highlights

- **Scene Discovery** — explore artists from the same city or musical scene; MusicBrainz-powered with genre filtering, A-Z grouping, and sidepanel discography view
- **Discovery Tabs** — redesigned Home with 3 tabs: customizable Home, Editor's Picks, and personalized For You with Radio Stations, Spotlight, and Rediscover Your Library
- **Audio engine upgraded** — vendored rodio/cpal removed; now on upstream rodio 0.22 with ALSA sample rate fallback, gapless on all backends, and smart quality downgrade
- **Booklet viewer** — albums with digital booklets get an in-app PDF viewer powered by native MuPDF; includes download button
- **3 Neon visualizers** — Laser, Tunnel, and Comet; all music-reactive with bass/mid/high response. Comet extracts colors from album artwork
- **Artist discovery** — MusicBrainz tag-based recommendations with tag-scoped thumbs down and similarity percentages
- **Label Releases redesigned** — logo header, sorting, filters, group-by-artist toggle, and search
- **Explicit badges** — shown across the entire app

## Scene Discovery

- Explore artists from the same city or musical scene — powered by MusicBrainz location and genre affinity
- Grid and sidepanel views with A-Z grouping, genre filter, and text search
- Sticky header with scene label and country flag on scroll

## Discovery Tabs (Redesigned Home)

- **3-tab home**: Home (customizable sections), Editor's Picks (editorial curation), For You (personalized)
- For You features Radio Stations, Artists to Follow, Spotlight artist, Rediscover Your Library, and Genre Essentials
- Sticky compact header with tab navigation and genre filter appears on scroll
- Progressive loading with session cache for instant back-navigation

## Audio

- ALSA sample rate fallback when DAC doesn't support requested rate
- PipeWire suspension before ALSA Direct exclusive access
- Gapless playback now available on ALSA Direct, defaults to ON
- Smart quality downgrade with hardware compatibility tooltip

## Immersive

- 3-mode background system (Full/Lite/Off) with auto-degrade on low FPS
- Per-panel FPS settings for each visualizer
- Comet visualizer adopts album art palette and fades on silence

## Security

- Removed hardcoded key material, cryptographic session IDs, DOM-based sanitization, log redaction

## Stability

- Graceful shutdown — no more heap corruption on exit
- Window size validation and clamping
- Flatpak tray icon and Snap MPRIS fixes

## Bug Fixes

- Equalizer bars no longer animate when paused
- Navigation scroll position scoped per item with 1-hour TTL
- Home settings apply immediately without reload
- Download paths corrected to Artist/Album/track structure
