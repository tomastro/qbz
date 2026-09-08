# QBZ 1.2.0 Release Highlights

## Highlights

- **Qobuz Connect** — full multi-device playback: renderer and controller mode, server-authoritative queue, bidirectional transport controls, session management, and device presence
- **Linebed visualizer** — 200-line 3D terrain driven by 4096-point FFT with real perspective projection at 60fps
- **Qobuz playlist follow/unfollow** — subscribe to playlists on your Qobuz account; new Favorites/Following sub-tabs
- **Audio settings sync overhaul** — all critical settings now sync to the player immediately; fixes exclusive mode not working with QConnect
- **Volume lock for ALSA Direct hw:** — software volume locked at 100% when bit-perfect hardware output is active
- **HiFi Wizard** — real DAC sample rates from /proc/asound; help tooltips on bit-perfect settings
- **MuPDF 0.6** — booklet viewer restored on Arch Linux
- **Offline mode** — Start Offline button on network error screen

## Qobuz Connect

- Renderer mode: receive playback commands from phone/tablet/web
- Controller mode: control remote devices from QBZ
- Server-authoritative queue sync across all devices
- QConnect badge in player bar with device type icon
- Developer panel and custom device name in Settings

## Linebed Visualizer

- 3D spectral terrain with 512 bands and logarithmic frequency redistribution
- Real camera projection with configurable FPS
- Canvas viewport adjusted to not overflow into player area

## Audio

- All audio setters (backend_type, exclusive_mode, dac_passthrough, etc.) now reload into CoreBridge player immediately
- Volume locked at 100% on ALSA Direct hw: — adjust at DAC level
- PipeWire rate switching: longer delay, post-creation verification, automatic retry
- HiFi Wizard reads real DAC rates from /proc/asound instead of hardcoded defaults

## Playlists

- Follow/unfollow Qobuz playlists (native API, syncs across clients)
- Favorites > Playlists split into Favorites and Following sub-tabs
- Bookmark button alongside Copy and Favorite in playlist detail

## Stability

- KWin SSD window rule hack removed — caused double titlebar and CPU spike
- Window size clamping fixed for multi-monitor setups
- Playback errors show actual messages instead of [object Object]
- Nix build credential tests skip correctly in sandbox

## Bug Fixes

- Offline mode accessible from network error screen
- Artist album counts removed (misleading Qobuz API data)
- MuPDF upgraded to 0.6 for Arch compatibility
- Snap MPRIS slot removed to unblock store review
- Issue templates migrated to YAML forms
