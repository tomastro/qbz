# QBZ 1.2.0 Full Changelog

185 commits between v1.1.20 and v1.2.0.

## New Features

### Qobuz Connect
- Full Qobuz Connect protocol implementation (renderer + controller)
- 4 new crates: qconnect-protocol, qconnect-core, qconnect-app, qconnect-transport-ws
- Native WebSocket transport with qcloud framing and protobuf encoding
- Queue domain model, reducer, and pending lifecycle
- Renderer domain state with server command contracts
- App adapter with pending timeout, resync, and concurrency policy
- Controller command API and session snapshots
- Admission middleware with blocked events
- QConnect toggle button beside cast in player bar
- QConnect panel with runtime diagnostics
- Renderer command routing through CoreBridge
- Queue materialization and remote playback hydration
- Active renderer state tracking for skip and seekbar
- Volume/mute routing to peer renderer
- Shuffle, repeat, queue play, and history play guards
- Autoplay mode sync and load_tracks remote commands
- QConnect badge as full-height badge in song card
- Developer toggle for QConnect dev panel
- Custom device name with hostname detection

### Linebed Visualizer
- New 3D terrain visualizer in Immersive mode
- 4096-point FFT with 512 bands and linear scaling
- 200 lines at 60fps with real 3D perspective projection
- Power-law frequency redistribution and edge tapering
- Isometric perspective matching musicvid.org camera angle
- FPS control in settings (default 60fps)
- Canvas viewport adjusted to not overflow player area

### Playlist Follow/Unfollow
- Qobuz API playlist/subscribe and playlist/unsubscribe endpoints
- v2_qobuz_subscribe_playlist and v2_qobuz_unsubscribe_playlist commands
- Bookmark toggle button in PlaylistDetailView
- Favorites/Following sub-tabs in Favorites > Playlists

### Other Features
- Documentation link in sidebar gear menu
- Help tooltips on bit-perfect audio settings (Exclusive Mode, DAC Passthrough, Force Bit-Perfect)
- No-telemetry note on MusicBrainz integration description
- Portuguese (pt-BR) translation
- Artwork proxy through backend cache
- Streaming playback with seekbar position tracking
- True gapless playback for ALSA Direct mode

## Audio Fixes

- Audio settings sync to CoreBridge player on change and session activation
- alsa_hardware_volume was hardcoded false in conversion function
- v2_reset_audio_settings now syncs to CoreBridge player
- Volume locked at 100% when ALSA Direct hw: is active
- PipeWire rate switch delay increased from 300ms to 500ms
- PipeWire rate verification after stream creation with automatic retry
- HiFi Wizard reads real DAC sample rates from /proc/asound
- Souvlaki MediaControlError formatting uses {:?} for cross-platform compatibility
- ALSA device lock prevented on streaming failure
- PipeWire suspension before ALSA Direct exclusive access
- Clock.force-rate re-applied after stream creation on resume
- HTTP auto-decompression disabled for audio downloads
- 120s total timeout removed for large Hi-Res+ tracks
- Quality fallback when CDN fails at current quality level
- Retry with fresh URL on CDN premature EOF

## Window / UI Fixes

- KWin SSD window rule hack removed entirely (caused double titlebar + CPU spike)
- Stale KWin rules from v1.1.14-v1.1.20 cleaned up on startup
- Screen resolution clamping fixed (xdpyinfo returns combined multi-monitor)
- Offline mode button added to network error screen
- Artist album counts removed from Favorites and Search (misleading API data)
- Playback error messages show actual details instead of [object Object]
- Discover menu items navigate to correct home tab
- Session restore hardened to prevent ghost CSS errors
- Tray icon visibility improved across Linux DEs
- Bit-perfect stream match rules corrected

## Distribution / Build

- MuPDF upgraded from 0.5 to 0.6 (fixes Arch Linux build)
- Booklet feature gate removed — MuPDF always compiled
- Snap MPRIS slot removed to unblock Snap Store review
- Issue templates migrated to YAML forms
- Dependency updates: devalue 5.6.4, SvelteKit 2.53.3
- Credential tests skip correctly in Nix sandbox
- Dependabot config and PR redirect workflow added
- Local library disc/CD suffix no longer misidentified as disc folders

## Qobuz Connect Protocol Details

- Protobuf queue command and server event contracts
- Android controller WebSocket message support
- Outbound renderer reports (RNDR_SRVR)
- Inbound server commands (SRVR_RNDR) decoding
- Queue drift reporting and reorder/remove event resyncs
- Authoritative queue mutations and shuffle order
- Remote repeat mode with pending/ack lifecycle
- Rapid transport control unblocking
- Session cursor alignment and renderer handoff
- Queue replacement through shared helper
- Stale renderer drift suppression during queue loads

## i18n

- All new features localized in 5 languages (en, es, de, fr, pt)
- Playlist follow/unfollow, sub-tabs, volume lock, help tooltips, documentation link, MusicBrainz privacy note
