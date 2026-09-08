# QBZ 1.1.19 Full Changelog

**Codename:** Limpiando la casa
**Commits:** ~200 since v1.1.18

---

## Audio Engine

### rodio 0.22 migration
- Removed vendored rodio/cpal fork entirely; now uses upstream rodio 0.22
- All decoder, stream, and buffer management updated to new API
- Explicit 100ms buffer size replaces vendored cpal tuning
- PipeWire clock.force-quantum disabled for rodio 0.22 compatibility

### ALSA improvements
- Sample rate fallback when DAC doesn't support the requested rate
- PipeWire sink suspended before ALSA Direct exclusive access to prevent DeviceBusy errors
- ALSA Direct gapless playback enabled and defaulted to ON
- Gapless restrictions removed for DAC Passthrough and ALSA Direct modes
- Pre-check hardware rates via /proc/asound before attempting ALSA Direct

### Smart quality downgrade
- When hardware can't handle a sample rate, QBZ selects the best compatible quality automatically
- Quality badge tooltip shows "Hardware compatibility" explanation
- Cached audio also verified against hardware rates before playback

### Other audio fixes
- Hot-reload gapless and normalization settings to active player
- Stop updating live gain during EBU R128 refinement passes
- Audio settings sync on startup for OAuth login path
- Output device name polling during playback
- Configured device name shown when player has no active stream
- Deprecated CPAL `name()` replaced with `description()`

---

## Booklet Viewer

- Albums with digital booklets show a booklet button in the album header
- Full in-app PDF viewer with page navigation
- MuPDF native backend replaces pdfjs-dist for crisp rendering and smaller bundle
- Download button to save booklet PDF locally
- Auto-fit page width on load
- V2 backend command fetches PDF through Tauri to bypass CORS
- Goody/booklet model added to qbz-models crate
- i18n keys added to all 4 locales

---

## Neon Visualizers

Three new Canvas 2D visualizers under Immersive > Neon:

### Laser (NeonFlowPanel)
- Neon laser beams that pulse, bend, and shift with bass/mid/high energy
- Dual rendering layers for glow effect

### Tunnel (TunnelFlowPanel)
- Depth-tunnel with warped elliptical rings
- Curved wisps flow outward from center
- Watercolor cloud layer for atmosphere

### Comet (CometFlowPanel)
- Watercolor nebula with curved wisps, tunnel rings, and central abyss
- Album art color extraction — samples artwork, derives 4 dominant hues + darkest color
- Palette lerps smoothly on track change
- Time-based hue mutation for continuous color evolution
- Fades to near-black during silence; wisps, rings, and clouds scale with total energy
- Corner vignettes using darkest album art color with bass-driven drift
- Beat detection spawns wisps on bass transients

### Shared
- All panels have per-panel FPS configuration
- Dedicated submenu with fixed positioning and hover bridge

---

## Immersive Mode

- **3-mode background** — Full (GPU shader), Lite (CSS blur), Off; configurable in settings
- **Auto-degrade** — sustained low FPS triggers automatic downgrade with dismissible modal notification
- **Per-panel FPS** — each visualizer has independent FPS settings; default 15 FPS
- **Configurable ambient FPS** — separate from panel FPS
- **Ambient background fixes** — 8x8 downscale replaces CSS blur; dark artwork mood preserved; MIRRORED_REPEAT for texture edges
- **Composition profiles** — GSK renderer selection and blur toggle per profile
- **GPU detection** — AMD and Intel identification for profile recommendations
- **GSK renderer control** — Cairo, GL, NGL, or Vulkan backend selection
- **Immersive settings** moved to end of Appearance section with standard collapsible pattern

---

## Artist Discovery

### MusicBrainz tag-based recommendations
- "You May Also Like" uses MusicBrainz genre tags instead of typical similar-artist APIs
- Deterministic shuffle for varied results across visits
- Shows 6 artists with 2 reserves for instant thumbs-down replacement
- Similarity percentage displayed per recommended artist

### Tag-scoped dismissals
- Thumbs down dismisses an artist from a specific tag/genre context only
- Same artist can still appear in different genre contexts
- SQLite table with composite key (tag + normalized artist name)
- Instant removal from list with reserve pull-in

### Artist Network sidebar
- Opens by default; preference persisted in localStorage
- State restoration after Queue/Lyrics close
- Clean transitions when navigating between artists (state reset on artist change)
- Simplified header: "Artist Network" title, PanelRightClose icon
- Empty relationship sections hidden automatically

---

## Label Releases View

- Completely redesigned with label logo header
- Sorting: date, title, popularity
- Filters: album, EP, single, live
- Search bar with animated expand
- Group-by-artist toggle with collapsible sections
- Performance improvements for large label catalogs
- Consistent back button and navigation height

---

## Security

- Removed hardcoded key material from credential encryption
- Cryptographically secure session ID generation (replaces deterministic)
- DOM-based HTML sanitization replacing regex-based sanitizer
- Sensitive identifiers and secrets redacted from backend logs
- Removed unused user_id accessors from Qobuz and Tauri API modules
- Hardened release-aur CI workflow permissions and input validation

---

## Spotify Import

- Removed Spotify API dependency entirely
- Uses embed page scraping for playlist import — no credentials needed
- Link resolver updated for new scraping approach

---

## New Features

- **Explicit content badges** across the entire app
- **Search track context menu** — right-click works on search result tracks
- **Track/disc number extraction** from file tags during local library import
- **Window size validation** prevents GDK crash from corrupt DB values (by @arminfelder, PR #136)
- **Cancel download** for purchase album downloads
- **Miniplayer keyboard shortcuts**
- **Autoconfig-graphics CLI tool** (`qbz --autoconfig-graphics`)
- **Custom artwork for playlists** and folder sidebar visibility controls
- **Image cache** — frontend image cache with size-aware loading, LRU eviction, and settings UI
- **Granular navigation history** with per-item scroll position

---

## Stability

- **Graceful shutdown** on RunEvent::Exit to prevent heap corruption
- **WebKit double-destroy prevention** — secondary windows close before main window
- **Window size clamping** to screen resolution; physical/logical pixel mismatch fixed
- **Flatpak tray icon** path corrected for sandbox environment
- **Snap MPRIS** declared as slot instead of plug
- **Titlebar double-click maximize** fixed
- **Opaque main window** default for shutdown stability on Linux
- **WebKit EGL shutdown workarounds** applied defensively

---

## Bug Fixes

- Equalizer animation no longer appears active when player is paused
- Home settings changes apply immediately without page reload
- Navigation scroll position scoped per item with 1-hour TTL
- Session restore no longer recalls stale scroll positions
- Download paths restructured to Artist/Album [FORMAT][Quality]/track
- EPs and singles backfilled when artist page omits them
- Search grid no longer collapses to single column on keystroke
- Escape on empty search input blurs focus instead of navigating
- Multi-select add-to-playlist sends all tracks in one call
- Deduplicate albums with quality subfolders in local library
- Reco meta caches cleared when image cache is cleared
- ML/reco module optimized for home view loading
- Artist resolution parallelized for card rendering
- Force dmabuf respected as full GPU opt-in for compositing
- FavQ and TopQ context types added to playback context
- Last.fm uses user-scoped storage for session restore
- Qobuz purchase source detection on library scan
- Svelte a11y and component warnings resolved across 26+ components
- ADR-001 compliance maintained across all new components

---

## Scene Discovery

- **Explore by location** — discover artists from the same city, country, or musical scene as any artist
- MusicBrainz-powered location and genre affinity matching with Qobuz catalog validation
- Progress tracking with animated loading phases (searching → validating → done)
- Two view modes: responsive grid and sidepanel with artist discography
- Alphabetical A-Z grouping with jump navigation index
- Genre multi-select filter with searchable popup (when >9 genres)
- Text search across discovered artists
- Sticky compact header on scroll with scene label, country flag, and genre summary
- Paginated "Load More" for large scenes
- Filtered count display (e.g., "42 / 100 artists")

---

## Discovery Tabs (Redesigned Home)

### 3-tab home layout
- **Home** — customizable feed with drag-to-reorder sections and per-section visibility toggles
- **Editor's Picks** — fixed Qobuz editorial curation (New Releases, Editor's Picks, Qobuzissimes, Press Awards, Popular Albums)
- **For You** — personalized recommendations built from listening history

### Home tab customization
- Sections include: New Releases, Press Awards, Popular Albums, Qobuzissimes, Editor's Picks, Qobuz Playlists, Your Mixes, Essential Discography, Recently Played, Continue Listening, Your Top Artists, More From Favorites
- Settings modal (gear icon) to toggle visibility and reorder sections
- Genre filter available on Home and Editor's Picks tabs

### For You tab
- Progressive loading with skeleton placeholders per section
- **Radio Stations** — 9 album-based radios (3 recent + 3 favorites + 3 top artists); dominant color extraction per card
- **Artists to Follow** — 10 suggested artists from similar-to-your-top-3 with styled 210px cards
- **Spotlight** — featured artist card with top tracks, playlists, "Create Radio" button, and "Qobuz Radio Station" subtitle; dominant color from artist image
- **Similar to [Album]** — albums related to a featured album
- **Rediscover Your Library** — neglected/forgotten albums from your collection
- **Essentials [Genre]** — essential albums from one of your top genres

### Sticky compact header
- Appears after 60px scroll; hides greeting and shows compact tab navigation with genre filter
- Edge-to-edge positioning with negative margin pattern (matching ArtistDetailView jump-nav)
- Time-based greeting (Morning/Afternoon/Evening) with i18n and username interpolation

### Session cache
- Album, artist, and track data cached to avoid redundant API calls on navigation
- Scroll position preserved per tab

---

## Packaging

- **Cargo.lock tracked in git** — enables deterministic builds for distro packagers (NixOS, etc.)
- **Tauri version sync** — @tauri-apps/api updated to 2.10.1 matching Rust crate 2.10.x
- **AUR cleanup** — removed non-functional aur-source and aur-git packages; only qbz-bin maintained
- **AUR maintainer email** updated
- **APT repository** — CI triggers APT repo update on release publish
- **Flatpak** — PipeWire stream matching restored; quality guidelines applied

---

## Contributors

- **@arminfelder** — Window size validation (PR #136)
