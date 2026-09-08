# QBZ v1.1.8 Highlights

## Release Overview

QBZ 1.1.8 is a major release with 431 commits and ~40,420 lines of net code growth. This release transforms QBZ from a desktop application into a multi-interface platform while delivering significant user-facing features.

## Key Highlights

### New Features

**Genre Filtering with 3-Level Hierarchy**
- Fine-grained genre filtering with parent/child/grandchild relationships
- Context-independent filters (home, favorites)
- Lazy loading and 30-day cache for performance
- Why it matters: Provides discovery capabilities beyond Qobuz's native client

**Playlist Suggestions Based on Vector Similarity**
- Algorithmic recommendations using artist vectors
- Integrates MusicBrainz relationships and Qobuz similarity data
- SQLite persistence with artist index mapping
- Why it matters: Introduces ML-adjacent discovery capabilities

**Immersive Player Complete Overhaul**
- Merges FocusView and FullPageView into unified experience
- WebGL-based ambient background with custom shaders
- Tab-based panel system (Static, Lyrics, Queue, Coverflow, Vinyl, TrackInfo, Suggestions, Visualizer)
- Why it matters: Modernizes playback experience with extensible architecture

**Audio Visualizer Without Bit-Perfect Compromise**
- Lock-free ring buffer taps samples from audio thread
- FFT processor on dedicated thread
- 16 frequency bars at 30 FPS
- Why it matters: Visualization features don't compromise audio quality

**DAC Setup Wizard for Bit-Perfect PipeWire Configuration**
- 10-step guided configuration for PipeWire
- DAC capabilities query from PipeWire/PulseAudio
- Distilled distro-specific commands (Debian, Arch, Fedora)
- Manual command execution prioritizes debuggability over automation
- Why it matters: Makes Linux audio configuration accessible to non-technical users

**Remote Control via REST API**
- Axum-based HTTP/HTTPS server with token authentication
- LAN-only access enforcement for security
- SSE endpoint for real-time playback updates
- OpenAPI specification for third-party integrations
- PWA frontend for remote control
- Why it matters: Establishes QBZ as a multi-interface platform

**Configurable Key Bindings**
- Centralized keyboard shortcuts system
- Conflict detection
- Category-based organization (Playback, Navigation, Interface, Focus Mode)
- Persistence to localStorage
- Why it matters: Improves discoverability and power user customization

**Artist Blacklist (Experimental)**
- SQLite-based blacklist with O(1) lookup performance
- Global enable/disable toggle
- Integration with search, radio, and queue operations
- Visual grayout for blacklisted artist tracks
- Why it matters: Provides user control over library content

### Improvements

**Audio Backend Architecture Refactor**
- Trait-based abstraction for PipeWire, ALSA, and PulseAudio
- Unified device enumeration and stream creation
- Backend manager factory pattern
- Enhanced device information (sample rates, device bus, hardware flag)
- Stable device IDs for persistent device selection
- Why it matters: Simplifies audio configuration and enables future backend support

**Enhanced Search**
- Combined `search_all` endpoint for albums, tracks, artists, playlists
- Improved album alignment with Qobuz
- Blacklist filtering in search results
- Why it matters: More efficient search with better results

**View Reworks**
- FavoritesView: Artist filtering, blacklist integration, improved performance (+2,023 lines)
- LocalLibraryView: Enhanced filtering, sorting, and search (+1,464 lines)
- ArtistDetailView: Blacklist integration, redesigned layout (~380 lines changed)
- SearchView: Improved results presentation and filtering (+1,044 lines)
- PlaylistDetailView: Suggestions integration, better track management (+444 lines)
- Why it matters: Modernized views with better UX and performance

**Theme and Visual Identity Refresh**
- New logo and iconography (QBZ logo, vinyl icon, Qobuz logo)
- Redesigned app icons (all sizes, 40-50% smaller)
- CSS overhaul with glass effects and improved contrast (+977 lines)
- Additional themes
- Why it matters: Improved brand differentiation and modern aesthetic

**Internationalization Expansion**
- French and German language support (+2,278 lines)
- Spanish expansion (+768 lines)
- English updates (+756 lines)
- Coverage for all new features
- Why it matters: Broader market coverage and improved accessibility

### Bug Fixes

**Stability**
- Fixed race conditions in fetching logic
- Resolved forced logout issues
- Fixed offline detection false positives
- Notification fallback when no system available
- Prefetch limit prevents memory corruption
- Resolved all Rust warnings

**Performance**
- Smarter image caching reduces excessive cache usage
- Genre filter cache with 30-day TTL
- Favorites cache for reduced API calls
- Reduced log noise (frequent commands moved to debug level)

**Audio Quality**
- Respect max quality limits without re-encoding (bit-perfect preserved)
- Better format negotiation with audio backends
- Stable device selection across restarts

**User Experience**
- Improved settings organization (removed unused settings)
- Better accessibility in contrast and themes
- Enhanced error handling and user feedback

## Technical Achievements

**Infrastructure Growth**
- Remote Control API server (1,200 lines backend + 1,189 lines OpenAPI)
- Artist vector system (2,000 lines across sparse vectors, store, builder, suggestions)
- Audio visualizer (600 lines with lock-free ring buffer and FFT processor)
- Reusable wizard components (800 lines for DAC setup)
- Audio backend abstraction (300 lines trait-based design)

**Platform Capabilities**
- Headless operation support via REST API
- Real-time events via SSE
- Third-party integration via OpenAPI spec
- Mobile control via PWA frontend
- ML-adjacent discovery via vector similarity

**Extensibility**
- Trait-based audio backends enable future backend support
- Tab-based immersive player enables new panels without core changes
- Reusable wizard components accelerate configuration flows
- Centralized key bindings simplify shortcut management

## Why This Release is Larger Than Expected

**Infrastructure Over Features**
The release evolved from user-facing features to major infrastructure expansion. Remote Control API required building HTTP server, documentation, and state synchronization—all beyond original scope.

**Architectural Prerequisites**
Playlist suggestions needed artist vector system (MusicBrainz integration, SQLite persistence, sparse vector math). DAC Setup Wizard needed reusable wizard components. These foundations weren't initially scoped.

**Cross-Cutting Concerns**
Remote Control API affected player state, queue management, search, and real-time events—requiring changes across multiple subsystems.

**Internationalization Effort**
Adding French and German support required translating all new features and existing UI (~2,300 lines of translation work).

## User Impact

**Discovery and Personalization**
- Genre filtering enables fine-grained library exploration
- Playlist suggestions provide algorithmic recommendations
- Artist blacklist allows content control

**Audio Experience**
- DAC Setup Wizard makes bit-perfect configuration accessible
- Audio visualizer adds visual feedback without compromising quality
- Enhanced audio backends simplify device management

**Usability**
- Key bindings customization improves power user experience
- Centralized shortcuts improve discoverability
- Modernized views with better performance and UX

**Platform**
- Remote control enables headless operation and mobile access
- Multi-interface support establishes QBZ as a platform
- Third-party integrations via OpenAPI spec

## Notable Design Decisions

**Manual DAC Configuration**
Wizard generates commands, user runs them manually.
- Prioritizes debuggability over automation
- Users see exactly what changes are made
- Reduces support burden from mysterious config changes

**LAN-Only Remote Control**
API only accepts connections from private IP addresses.
- Security: Prevents unauthorized public access
- Simple token authentication is sufficient
- Users need VPN or tunnel for outside access

**Lock-Free Visualizer**
Visualizer uses lock-free ring buffer with relaxed atomic ordering.
- No locks or copies in audio path
- Preserves bit-perfect playback
- Tolerates slightly stale data (acceptable for visualization)

**Experimental Features**
Artist blacklist and remote control are opt-in with feature flags.
- Limited QA can be managed
- Users can opt-in if they want the feature
- Reversible if problems arise

## Easy-to-Overlook Changes

**Remote Control API OpenAPI Spec**
- Establishes public API contract for third-party integrations
- Enables auto-generated client SDKs
- Documents all endpoints, parameters, and responses

**Audio Backend Abstraction Trait**
- Decouples audio logic from specific backends
- Enables adding new backends without modifying calling code
- Provides unified interface for device enumeration

**Stable Device IDs**
- Device selection persists across app restarts
- No need to re-select device after USB disconnect/reconnect
- Reduces user friction for multi-DAC setups

**Genre Filter Persistence**
- Filters survive app restarts (localStorage)
- Context-independent state (different filters for home, favorites)
- 30-day cache reduces API calls

**Notification Fallback**
- Works on systems without notification support
- Graceful degradation (no crashes or errors)
- Better experience on minimal Linux installs

## Alignment with Long-Term Vision

**Strengths**
- Platform architecture enables headless operation and third-party integrations
- Audio quality preserved through lock-free design and backend abstraction
- Extensible foundations (trait-based backends, tab-based player, reusable wizards)
- Performance improvements (cache, stable device IDs, prefetch limits)
- User control prioritized over automation (manual DAC config, user-selected backends)

**Areas for Future Attention**
- Code growth (+40,420 lines) requires consolidation and cleanup
- Increased complexity needs better documentation and testing coverage
- Feature bloat evaluation (consider opt-in features for casual users)
- Internationalization maintenance (4 languages increases translation burden)

## Conclusion

QBZ 1.1.8 is a landmark release that transforms QBZ from a desktop application into a multi-interface platform with sophisticated audio capabilities. The release delivers on its user-facing features while building substantial infrastructure for future growth.

The release's size is justified by the architectural foundations it establishes. However, future releases should focus on consolidation, testing, and documentation to address the increased maintenance burden.

Overall, QBZ 1.1.8 is aligned with long-term technical and product vision, provided that future releases address the growth and complexity concerns raised by this release's scope.

---

## Release Metrics

- **Commits**: 431 from main to pre-release
- **Files changed**: 243 files
- **Lines added**: 43,498
- **Lines removed**: 3,078
- **Net growth**: ~40,420 lines
- **Approximate size**: 3-4x growth of typical QBZ releases

## Key Files Added/Modified

**New Rust Modules** (~6,600 lines)
- `src-tauri/src/api_server/` - Remote Control API
- `src-tauri/src/artist_vectors/` - Vector similarity system
- `src-tauri/src/artist_blacklist/` - Blacklist implementation
- `src-tauri/src/visualizer/` - Audio visualizer
- `src-tauri/src/audio/backend.rs` - Backend abstraction

**New Frontend Components** (~9,000 lines)
- `src/lib/components/immersive/` - Immersive player overhaul
- `src/lib/components/DACSetupWizard.svelte` - DAC configuration
- `src/lib/components/wizard/` - Reusable wizard components
- `src/lib/components/GenreFilter*.svelte` - Genre filtering
- `src/lib/components/PlaylistSuggestions.svelte` - Suggestions UI

**New Stores** (~1,200 lines)
- `src/lib/stores/keybindingsStore.ts` - Key bindings
- `src/lib/stores/genreFilterStore.ts` - Genre filtering
- `src/lib/stores/artistBlacklistStore.ts` - Blacklist state

**Configuration** (~1,000 lines)
- `src-tauri/src/config/remote_control_settings.rs`
- `src-tauri/src/config/favorites_cache.rs`
- `src-tauri/src/config/playback_preferences.rs`
- `src-tauri/src/config/legal_settings.rs`

**Documentation** (~1,200 lines)
- `docs/openapi.yaml` - Remote Control API specification

**Internationalization** (~3,800 lines)
- `src/lib/i18n/locales/de.json` - German (new)
- `src/lib/i18n/locales/fr.json` - French (new)
- `src/lib/i18n/locales/es.json` - Spanish (expanded)
- `src/lib/i18n/locales/en.json` - English (expanded)

**Visual Assets**
- New logos and icons (QBZ logo, vinyl icon, Qobuz logo)
- Redesigned app icons (all sizes)
- Static SVG assets for UI elements

**Major View Reworks** (~5,000 lines)
- `src/lib/components/views/FavoritesView.svelte` (+2,023 lines)
- `src/lib/components/views/LocalLibraryView.svelte` (+1,464 lines)
- `src/lib/components/views/SearchView.svelte` (+1,044 lines)
- `src/lib/components/views/SettingsView.svelte` (+1,669 lines)
- `src/lib/components/views/PlaylistDetailView.svelte` (+444 lines)

## Credits

This release involved significant contributions from multiple contributors, including:
- Core audio architecture and visualizer
- Remote Control API implementation
- Artist vector system and ML integration
- DAC Setup Wizard development
- Internationalization (French, German, Spanish)
- UX/UI design and theme refresh
- Bug fixes and performance improvements

## Next Steps

**Short-term**
- Monitor stability of experimental features (remote control, blacklist)
- Gather user feedback on DAC Setup Wizard usability
- Evaluate genre filtering adoption and performance

**Medium-term**
- Consolidate duplicate code from this release
- Increase test coverage for new modules
- Add architecture documentation for complex systems

**Long-term**
- Extend artist vector system for more ML features
- Expand remote control API capabilities
- Consider additional audio backends (JACK, CoreAudio)
- Evaluate adding more languages
