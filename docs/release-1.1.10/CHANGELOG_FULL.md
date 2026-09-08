# QBZ 1.1.10 Full Changelog

This document details all changes introduced in version 1.1.10 after the commit that would have become the 1.1.10 tag-release on main (e48bbb99, dated 2026-02-07).

**Summary:** 150 files changed, 17,049 insertions(+), 2,827 deletions(-) across 178 commits

---

## Fixes

### Debugging & Defensive Fixes

- **Search UI freeze (#56)**
  - Frontend: Yield to UI loop with `tick()` after setting search results before running background download status checks
  - Backend: Drop client mutex lock immediately after HTTP request completes so parsing and blacklist filtering don't hold the lock
  - Prevents complete UI blockage during search operations

- **Startup blocking**
  - Scope client mutex locks properly and yield before download status checks
  - Prevents startup delay on large libraries

- **EGL crash prevention**
  - Force X11 backend on Wayland for AppImage builds to prevent WebKitGTK EGL crashes
  - Pin WebKitGTK to 2.44.0-2 in AppImage
  - Disable WebKit GPU compositing to fully prevent EGL issues
  - Auto-detect virtual machines and force software rendering
  - Add `--reset-dmabuf` CLI flag for crash recovery

- **Large playlist UI blocking**
  - Virtualize playlist track list using absolute positioning and viewport culling
  - Only ~20 track rows in DOM at any time instead of all 2000
  - Eliminates main-thread blocking when navigating between playlists

- **Lyrics reliability**
  - Retry LRCLIB once on network error before fallback
  - Fallback to lyrics.ovh when LRCLIB fails
  - Search-first strategy to prioritise synced lyrics from LRCLIB
  - Re-fetch when cached entry lacks synced lyrics

- **Spotify playlist import**
  - Add token retry logic
  - HTTP status checking and diagnostics
  - Improved pagination reliability
  - Stop silent embed fallback when API fetch fails
  - Restore embed fallback only when API fetch explicitly fails

- **Playlist duplicate warnings**
  - Added confirmation modal when adding duplicate tracks to playlists
  - Similar copy to official iOS client
  - Contribution by @vorce (commit 512a4112)

- **PostCSS extraction errors**
  - Eradicate all `t`-as-lambda-param shadowing across codebase
  - Remove `$t()` calls from `$derived()` blocks
  - Remove store accesses from `$derived()` blocks

- **Favorites view positioning**
  - Prevent content from overlapping sticky alpha-index
  - Fix back button positioning and async lint error
  - Move back button above header as standalone element

- **z-index layering**
  - Remove z-index from artist page sticky nav to prevent sidebar dropdown overlap
  - Lower favorites-nav z-index to prevent overlap
  - Remove z-index from dropdown-container
  - Bump jump-nav z-index above scrolling content

- **Playlist suggestions**
  - Skip at Qobuz limit (2000 tracks)
  - Manual launch for large playlists
  - Add bottom spacer when suggestions section is absent

- **Navigation issues**
  - Fix track not showing as active after session restore
  - Update badge status immediately when playback starts

- **Other defensive fixes**
  - Remove invalid artistId/onArtistClick props from AlbumCard usage
  - Fix toggle props for Force DMA-BUF (enabled/onchange, not checked/onChange)
  - Restore isBlacklisted prop in AlbumDetailView TrackRow
  - Fix hover background flicker on download button
  - Fix 0x0.st upload 403 by adding User-Agent header
  - Gate launch modals on session readiness
  - Ensure preferences are loaded from backend after login
  - Restore token and enabled state for ListenBrainz during session init
  - Clear old tracks immediately on playlist switch to prevent stale recomputation

### Performance

- **Playlist fetching (large playlists ~2000 tracks)**
  - Two-phase progressive loading: track_ids first (~50KB), then track/getList in batches of 50
  - 4 concurrent batch groups with progressive rendering
  - Adaptive loading: single call for small playlists, progressive for large
  - Virtualization: viewport culling renders only visible tracks
  - Keep spinner until first viewport batch loads
  - Eliminate placeholders and proxy bloat for large playlists
  - Use `$state.raw` for tracks to eliminate proxy bloat
  - Clear old tracks immediately on playlist switch

- **Home view loading**
  - SWR cache with stale-while-revalidate eliminates loading delay on back-navigation
  - Replace ~50 IPC calls with single `reco_get_home_resolved` invoke
  - 3-tier metadata cache (reco meta → API cache → Qobuz API)
  - Server-side resolution eliminates per-item fetch loop on frontend
  - Add `homeDataCache` store for instant Home navigation

- **Sidebar playlists**
  - SWR cache with stale-while-revalidate
  - Virtualized playlist list prevents UI freeze

- **Artist view component rendering**
  - VirtualizedFavoritesArtistGrid component (virtualization for artist grids)
  - VirtualizedFavoritesArtistList component (virtualization for artist lists)
  - VirtualizedFavoritesAlbumGrid component (virtualization for album grids)
  - Inline virtualization for SearchView results
  - Integration into FavoritesView and sidepanel

- **Playlist Manager View**
  - Fix O(n^2) findIndex in render loop: replace with pre-computed Map<id, index> for O(1) lookups
  - Optimize filter pipeline

- **Download status checks**
  - Batch album download status checks for better performance

- **API client**
  - Replace Mutex with RwLock for QobuzClient for improved concurrent read performance

- **Search**
  - Prevent UI freeze during search operations

- **Playlist import**
  - Concurrent matching with progress reporting
  - Paginate get_playlist to fetch all tracks

## UI/UX Improvements

### Playlist Management

- **Playlist duplicate warnings**
  - Confirmation modal when adding duplicate tracks to playlists
  - Similar copy to official iOS client
  - Contribution by @vorce

- **Playlist image preservation**
  - Preserve original Qobuz playlist image when copying to library

- **Back-to-top button**
  - Added for large playlists
  - Positioned 10px above the player bar
  - Moved to global layout level for reuse

- **Playlist suggestions**
  - Defer computation for large playlists (>500 tracks)
  - Manual launch button in PlaylistDetailView
  - Skip computation at Qobuz limit (2000 tracks)

### Navigation & Scrolling

- **Scroll position memory**
  - Added to all detail views
  - Navigation remembers position when returning to views

- **Back button**
  - Added to FavoritesView
  - Moved above header as standalone element
  - Fixed positioning issues

- **See All links**
  - Moved below section title instead of beside it
  - Replace pipe separator with arrow icon
  - Added to all discover sections in HomeView

### Tooltips & Badges

- **Custom tooltip system**
  - Global system replacing native title tooltips
  - Styled and consistent across application
  - Match `data-sys-tooltip` on subsequent hovers

- **Quality badge improvements**
  - Hardware rate detection
  - Custom structured tooltips
  - Explain degraded quality (resampling) and how to fix it
  - Differentiate DAC Passthrough tooltip from ALSA Direct bit-perfect
  - Reduce height in AlbumCard, restore 90px width

- **Artist name navigation**
  - Clickable artist name to AlbumCard across all views
  - Wire artist click through all views using AlbumCard

### Window & Appearance

- **System title bar toggle**
  - Add toggle in Settings > Appearance
  - Restart app on toggle
  - Only restart when enabling system titlebar
  - Set system decorations before window show
  - Create main window programmatically for correct decorations
  - Hide window until webview renders to avoid empty frame
  - Use dark background instead of hidden window
  - Sync localStorage value to Rust backend on init

- **Hardware acceleration**
  - Add toggle in Settings > Appearance
  - Disabled by default for AppImage compatibility
  - Add graphics settings backend for opt-in
  - Env var `QBZ_HARDWARE_ACCEL=1|0` always overrides DB value
  - Force software rendering when hardware accel is disabled

### Settings & Developer Tools

- **Developer Mode**
  - New section in Settings
  - Fix Factory Reset always visible bug
  - Add i18n keys for Developer Mode section in all 4 locales
  - Add developer settings backend and log capture system
  - Add `--reset-dmabuf` CLI flag

- **Log management**
  - LogsModal with per-tab upload
  - URL display with copy functionality
  - Bug report hint
  - Show uploaded log URL next to upload button

- **Factory reset**
  - Reset Audio & Playback settings to defaults
  - Add `reset_audio_settings` Rust command
  - Add `factory_reset` Rust command
  - Move streaming settings to Playback section
  - Change playback defaults: gapless off, context icon off, stream uncached off

### UI Polish

- **Loading indicators**
  - Replace flickering Loader spinner with SVG progress ring
  - Keep spinner until first viewport batch loads in playlists

- **Discover sections**
  - Move See All link below section title
  - Replace pipe separator with arrow icon
  - Add to all discover sections

- **Genre filter**
  - Use existing genre filter in discover browse pages
  - Fix ranking display and See All positioning
  - Add genre filter dropdown to DiscoverBrowseView

- **Playlist tags**
  - Use localized playlist tags
  - Align tag selector below section title

- **Search tabs**
  - Remove inner scroll, use parent scroll container

- **Favorites view**
  - Move edit button to top bar inline with back button
  - Optimize render loop with Map-based lookups

### Discover Module Enhancements

- **Discover browse pages**
  - New Releases, Ideal Discography, Top Albums
  - Qobuzissimes, Albums of the Week, Press Accolades
  - Playlist tags browse page
  - Per-category releases pagination via get_releases_grid

- **See All entry points**
  - Added to all 6 discover sections in HomeView
  - Navigate to dedicated browse pages
  - Chevron caret and pipe separator

- **Ranking display**
  - Top-right position
  - Impact font
  - 35px size

### Home View Improvements

- **Continue Listening**
  - Wire download button action in tracks

- **Performance**
  - SWR cache with stale-while-revalidate
  - Consolidated API calls from 6 to 1 via discover/index
  - Single IPC call with 3-tier metadata cache

- **Navigation**
  - Integrate home data cache for instant back-navigation

### Artist Page Refactor

- **API integration**
  - Add Rust backend for artist/page and releases/grid endpoints
  - Add TypeScript types and adapter for artist/page endpoint
  - Integrate artist/page endpoint into navigation
  - Per-category releases pagination via get_releases_grid

- **Image handling**
  - Correct artist portrait image URL construction
  - Remove unused imports of convertQobuzArtist and appendArtistAlbums

### Local Library View

- **Plex integration**
  - Redesign settings flow and gate local library integration
  - Hydrate track list quality without blocking UI
  - Offline mode support
  - Background quality hydration
  - Single-file detection

### Other UI improvements

- **Window decorations**
  - Fix window always restart on system titlebar toggle
  - Only restart when enabling system titlebar

- **Download button**
  - Remove hover background flicker

## Features

### Gapless Playback

- **Implementation**
  - PipeWire only (Rodio backend)
  - Reliable only when tracks share same sample rate/channels
  - Cached data only
  - Disabled for ALSA Direct, streaming, and user skips
  - Falls back to normal stop-play flow when ineligible

- **Backend**
  - Add `PlayNext` command to audio thread
  - Append decoded sources to existing Rodio Sink
  - Detect when ~5s remain and signal `gapless_ready` via SharedState
  - Position-based transition detection

- **Frontend**
  - Detect `gapless_ready` flag
  - Invoke `play_next_gapless` (cache-only lookup)
  - Advance queue without stop/play cycle
  - `gaplessTransition` option to skip stop_playback when backend already transitioned

- **UI**
  - Add toggle button to player bar
  - Later moved to Settings > Playback

### Volume Normalization (EXPERIMENTAL)

- **Phase 1: ReplayGain metadata**
  - Extract ReplayGain track gain/peak tags from audio file metadata
  - Support Vorbis comments and ID3v2 TXXX tags
  - Apply gain factor via rodio's Amplify wrapper
  - Clipping prevention using peak metadata
  - Gain preserved across resume/seek operations

- **Pipeline**
  - Diagnostic (raw) -> Amplify (normalization) -> Visualizer
  - When OFF (default): no Amplify wrapper, 100% bit-perfect

- **Settings**
  - `normalization_enabled`, `normalization_target_lufs`
  - Supports -14 LUFS (default), -18 LUFS, -23 LUFS targets
  - Commands: `set_audio_normalization_enabled`, `set_audio_normalization_target`

- **Phase 2: EBU R128 real-time**
  - Real-time EBU R128 volume normalization
  - Loudness analyzer and cache
  - Dynamic amplification
  - Expose normalization gain in playback events

- **UI**
  - Add toggle to player bar
  - Hide normalization icon until Phase 2

### Plex Integration (EARLY STAGE)

- **Overview**
  - First iteration
  - Connect to local or LAN Plex servers
  - Browse and play local library tracks
  - Explicitly marked as early-stage / incomplete

- **Backend**
  - New module: `src-tauri/src/plex/mod.rs` (1405+ lines)
  - Plex API integration
  - Quality detection and metadata
  - Track list quality hydration

- **Settings**
  - Redesign settings flow
  - Gate local library integration
  - Plex metadata write toggle

- **Features**
  - Offline mode
  - Background quality hydration
  - Single-file detection
  - Preserve hydrated quality during re-sync
  - Persist hydrated quality metadata in cache

- **UI**
  - Local Library View enhancements
  - Quality indicator
  - Plex source indicator
  - Settings integration

### Discover Browsing

- **Browse pages**
  - New Releases (newReleases)
  - Ideal Discography (idealDiscography)
  - Top Albums (mostStreamed)
  - Qobuzissimes (qobuzissimes)
  - Albums of the Week (albumOfTheWeek)
  - Press Accolades (pressAward)
  - Playlist tags (with genre_ids support)

- **Backend**
  - Add `get_discover_albums` Tauri command
  - Paginated API calls with genre_ids filtering
  - Per-category releases pagination via get_releases_grid

- **Frontend**
  - DiscoverBrowseView component
  - Virtualized grid/list
  - Genre filter dropdown
  - Client-side search
  - Infinite scroll
  - VirtualizedFavoritesAlbumGrid enhancements (showRanking, onLoadMore, loading indicator)

- **Navigation**
  - "See All" links from HomeView sections
  - Dedicated view types for each section
  - Genre filter contexts

### Playlist Import

- **Large playlist support**
  - Automatic multipart playlists for 2000+ track imports
  - Split into "Playlist (Part 1)", "Playlist (Part 2)", etc.
  - Move all parts to folder
  - Show parts count in summary

- **Performance**
  - Concurrent matching with progress reporting
  - Paginate get_playlist to fetch all tracks

- **UI**
  - Progress reporting during import
  - Parts count display

### Other Features

- **Smart playlists**
  - Remove total track limit from smart playlists

- **Audio diagnostics**
  - Internal bit-depth diagnostic capture
  - Diagnostic logging system

- **Home resolved API**
  - Replace ~50 IPC calls with single invoke
  - 3-tier metadata cache

- **Artist page endpoint**
  - Rust backend for artist/page and releases/grid
  - TypeScript types and adapter
  - Integration into navigation

## Internal Changes

### Refactoring

- **Home API consolidation**
  - Consolidate 6 API calls to 1 via discover/index
  - Add `reco_get_home_resolved` Rust command

- **Client API**
  - Replace Mutex with RwLock for QobuzClient
  - Add POST fallback and flexible response parsing for track/getList
  - Add genre_ids support to discover playlists endpoint

- **Playlist loading**
  - Rewrite loadPlaylist() for two-phase progressive loading
  - Show playlist header + placeholder tracks after Phase 1
  - Update track removal to handle missing playlist_track_id

- **Virtualization**
  - Playlist track list with viewport culling
  - Sidebar playlist list
  - Favorites view components
  - SearchView results

### Code Quality

- **i18n improvements**
  - Replace hardcoded strings in PlaylistModal
  - Add missing i18n keys (e.g., actions.playNext)
  - Avoid callback shadowing in local library view
  - Add checker for t shadowing in svelte files

- **TypeScript**
  - Add type definitions for new features
  - Fix async lint errors

- **Vendor fixes**
  - Fix unused import and mut warnings in rodio
  - Fix stream.rs in vendor/rodio

### Documentation

- **Issue templates**
  - Update with installation method and log instructions
  - Expand crash log instructions for all installation methods

### Build & Packaging

- **Version bump**
  - Bump version to 1.1.10

- **AUR**
  - Add pkgrel input to AUR workflow
  - Update PKGBUILD version to 1.1.9 (note: this was for 1.1.9 release)

- **Snap**
  - Update snapcraft metadata for 1.1.9 (note: this was for 1.1.9 release)

- **Flatpak**
  - Update metadata

- **GitHub Actions**
  - Update release workflows

### Debugging & Diagnostics

- **Timing logs**
  - Add timing logs across entire playlist load pipeline
  - Log playlist name on load for virtualization tracking

- **Diagnostic logging**
  - Add aggressive logging to home loading flow
  - Add API response status logging and 404 handling
  - Add catch handlers to all home section promise chains
  - Remove DEBUG-43 diagnostic logging after development

### Settings Architecture

- **New settings modules**
  - Audio settings: Reset functionality
  - Developer settings: Debug mode, log capture
  - Graphics settings: Hardware acceleration
  - Window settings: Decorations, titlebar
  - Playback preferences: Gapless, normalization, etc.

### Store & State Management

- **New stores**
  - `consoleLogStore`: Log capture and management
  - `sidebarDataCache`: Sidebar playlist caching
  - `titleBarStore`: Title bar state

- **Store updates**
  - `homeDataCache`: Enhanced with stale-while-revalidate
  - `playerStore`: Gapless playback, normalization gain
  - `queueStore`: Plex integration
  - `navigationStore`: New view types for discover
  - `genreFilterStore`: Genre contexts for discover
  - `updatesStore`: Preference loading

### API Endpoints

- **New endpoints**
  - `TRACK_GET_LIST`: Batch track fetching
  - Artist page and releases grid
  - Discover albums (various types)
  - Playlist tags with genre_ids

- **Enhanced endpoints**
  - Playlist: extra=track_ids for lightweight metadata fetch
  - Discover: genre_ids filtering support

### Database & Caching

- **API cache**
  - 3-tier metadata cache for home resolved
  - SWR pattern for home and sidebar

- **Reco store**
  - Enhanced with new browse endpoints
  - Metadata caching improvements

- **Session store**
  - ListenBrainz token restoration
  - Plex server info

### Logging System

- **New module**
  - `src-tauri/src/logging.rs`: Log capture and management

- **Features**
  - Console log capture
  - Per-tab log upload
  - URL display and copy
  - User-Agent header for uploads

### Audio Pipeline Enhancements

- **New modules**
  - `audio/loudness.rs`: ReplayGain extraction and calculation
  - `audio/loudness_analyzer.rs`: EBU R128 loudness analysis
  - `audio/loudness_cache.rs`: Loudness metadata cache
  - `audio/analyzer_tap.rs`: Diagnostic tap
  - `audio/dynamic_amplify.rs`: Dynamic amplification
  - `audio/diagnostic.rs`: Audio diagnostics

- **Pipeline changes**
  - Support for Amplify wrapper (normalization)
  - Gapless playback queueing
  - Diagnostic capture point

### Playback Enhancements

- **New commands**
  - `play_next_gapless`: Queue next track for gapless
  - `set_audio_normalization_enabled`: Toggle normalization
  - `set_audio_normalization_target`: Set target LUFS
  - `reset_audio_settings`: Reset audio and playback preferences
  - `factory_reset`: Full factory reset
  - `get_discover_albums`: Fetch discover albums
  - Various Plex commands

- **Playback service**
  - Gapless transition support
  - Normalization gain handling
  - Plex integration

- **Player**
  - Gapless queue management
  - Position-based transition detection
  - Normalization pipeline integration

### Commands Updates

- **Playlist commands**
  - Progressive loading support
  - Track removal with playlist_track_id handling

- **Search commands**
  - Mutex lock improvements
  - Discover album fetching

- **Favorites commands**
  - Minor adjustments

- **Other commands**
  - Credits, musician, radio updates
  - Share command fixes
  - Smart playlist enhancements
  - Offline cache migration
  - Cast/DLNA fixes

---

## Translation Updates

All four locales (en, es, de, fr) updated with:
- Actions (playNext, etc.)
- Developer Mode section
- Discover module enhancements
- Playlist import improvements
- Plex integration
- Settings reorganization
- Audio/Playback settings
- Quality badge tooltips
- Log management

---

## Assets Added

- SVG icons: `bars-disorder-outlined.svg`, `bars-normal.svg`
- Plex logos: `plex-logo.svg`, `plex-mono.svg`
- Updated: `home-gear.svg`

---

## Dependencies

- Updated package-lock.json
- Minor updates to package.json

---

## Build System

- Added `scripts/check-no-t-shadow.sh` for i18n linting

---

## Summary by Category

| Category | Commits | Files Changed | Insertions | Deletions |
|----------|---------|--------------|------------|-----------|
| Performance | ~35 | ~50 | ~4,000 | ~800 |
| UI/UX | ~60 | ~80 | ~6,500 | ~1,200 |
| Fixes | ~50 | ~60 | ~3,000 | ~500 |
| Features | ~25 | ~35 | ~3,000 | ~200 |
| Internal | ~8 | ~15 | ~549 | ~127 |
| **Total** | **178** | **150** | **17,049** | **2,827** |

---

## Notable Contributors

- @vicrodh (maintainer): 177 commits
- @Joel C (vorce): 1 commit (Playlist duplicate warning modal)

---

## Breaking Changes

None. All changes are backward compatible.

---

## Known Limitations

- **Gapless playback**: PipeWire only, requires same sample rate and cached tracks
- **Plex integration**: Early stage, incomplete feature set
- **Volume normalization**: Experimental, may affect audio quality

---

## Testing Recommendations

1. **Large playlists**: Test with 2000+ track playlists for performance
2. **Search**: Verify no UI freeze during search operations
3. **Gapless playback**: Test with same sample rate tracks on PipeWire
4. **Volume normalization**: Test with ReplayGain-tagged files
5. **Plex integration**: Test with local and LAN servers
6. **Wayland**: Verify AppImage works correctly on Wayland compositors
7. **Discover browsing**: Test all browse pages and genre filters
8. **Playlist import**: Test with large Spotify playlists (2000+ tracks)

---

## Next Steps

Consider for 1.1.11 or future releases:
- Complete gapless playback for ALAC Direct
- Improve volume normalization accuracy
- Expand Plex integration feature set
- Additional discover module enhancements
- Further performance optimizations
