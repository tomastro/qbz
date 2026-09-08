# QBZ 1.1.10 Release Highlights

## Fixes

- **Search UI freeze**: Resolved complete interface freezing during search operations by implementing proper async handling and mutex lock management
- **EGL crashes on Wayland**: Fixed WebKitGTK crashes when running AppImage on Wayland compositors through proper X11 backend forcing
- **Virtual machine rendering**: Auto-detected VMs and forced software rendering to prevent crashes in virtualized environments
- **Large playlist performance**: Eliminated UI freezes when loading playlists with thousands of tracks
- **Lyrics reliability**: Added LRCLIB retry on network errors and fallback to lyrics.ovh when primary source fails
- **Playlist duplicate warnings**: Added confirmation dialog when adding duplicate tracks to playlists (contribution by @vorce)

## Performance

- **Playlist loading**: Progressive two-phase loading (metadata first, track details in batches) reduces initial load time from seconds to milliseconds
- **Large playlists**: Virtualization renders only visible tracks (~20 in DOM) instead of all 2000+, preventing UI freezes
- **Home view**: SWR caching with stale-while-revalidate eliminates loading delay when returning to home; consolidated 6 API calls into 1
- **Sidebar playlists**: Virtualized playlist list prevents UI freeze when library contains many playlists
- **Favorites view**: Virtualized artist/album grids and lists for smooth scrolling with large libraries
- **Search results**: Inline virtualization prevents UI freeze when displaying many search results
- **Playlist import**: Concurrent track matching with progress reporting for faster imports
- **API client**: Replaced Mutex with RwLock for improved concurrent read performance

## UI/UX Improvements

- **Playlist copy to library**: Preserves original Qobuz playlist image artwork
- **Back-to-top button**: Appears on long playlists for quick navigation
- **Scroll position memory**: Detail views remember scroll position when navigating back
- **Back button in Favorites**: Added for consistent navigation
- **Custom tooltips**: Replaced native title tooltips with styled global system
- **Quality badge improvements**: Hardware rate detection and structured tooltips explaining resampling
- **Artist name navigation**: Clickable artist names in album cards across all views
- **System title bar**: Toggle between custom and system window decorations
- **Discover sections**: "See All" links for full-page browsing of New Releases, Ideal Discography, Top Albums, Qobuzissimes, Albums of the Week, and Press Accolades
- **Playlist tags**: Filter and browse Qobuz's curated playlists by tag
- **Developer Mode**: New settings section for advanced debugging and log management
- **Log sharing**: LogsModal with per-tab upload, URL display, and copy functionality for easy bug reporting
- **Factory reset**: Reset Audio & Playback settings to defaults without clearing library

## Features

- **Gapless playback** (PipeWire only, experimental): Seamless transitions between tracks when same sample rate and cached
- **Volume normalization** (experimental): EBU R128 ReplayGain-based volume leveling with clipping prevention
- **Plex integration** (early stage): Connect to local or LAN Plex servers and browse/play local library tracks
- **Discover browsing**: Full-page views for all Qobuz Discover modules with genre filtering and infinite scroll
- **Playlist import improvements**: Support for 2000+ track imports via automatic multipart playlists
