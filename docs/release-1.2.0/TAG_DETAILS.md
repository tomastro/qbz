# 1.2.0 — Qobuz Connect

The headline feature of this release is **Qobuz Connect** — a full implementation of Qobuz's real-time streaming protocol that turns QBZ into both a renderer and controller for multi-device playback. Also includes the **Linebed visualizer**, **Qobuz playlist follow/unfollow**, a complete audio settings sync overhaul, and dozens of fixes across playback, offline mode, and window management.

---

## Qobuz Connect

QBZ now implements the Qobuz Connect protocol, enabling multi-device playback control. Start music on your phone, hand it off to QBZ on your desktop, or control QBZ from any Qobuz client.

  - **Renderer mode** — QBZ receives playback commands from other Qobuz clients (phone, tablet, web)
  - **Controller mode** — control playback on remote devices from QBZ's UI
  - **Server-authoritative queue** — queue state is synced through Qobuz's cloud, ensuring consistency across all devices
  - **Transport controls** — play, pause, skip, seek, shuffle, repeat, and volume all work bidirectionally
  - **Session management** — join/leave sessions, renderer selection, and device presence
  - **QConnect badge** — compact two-row badge in the player bar showing connection status and active device type
  - **Developer panel** — optional diagnostics panel for QConnect debugging (hidden by default, toggle in Settings > Developer Mode)
  - **Custom device name** — editable announce name in Settings > Integrations (defaults to "Qbz - hostname")

Built from scratch as 4 independent crates: `qconnect-protocol` (protobuf wire format), `qconnect-core` (queue/renderer domain), `qconnect-app` (application logic), and `qconnect-transport-ws` (WebSocket transport).

## Linebed Visualizer

A new 3D terrain visualizer in Immersive mode, inspired by musicvid.org's spectral landscape.

  - **200 lines at 60fps** — real-time 3D mountain terrain driven by audio spectrum data
  - **4096-point FFT** — 512-band spectral analysis with logarithmic frequency redistribution
  - **Real 3D projection** — proper camera, perspective, and depth with configurable FPS
  - **Power-law scaling** — spectral peaks are redistributed for visual balance across bass, mids, and highs

## Qobuz Playlist Follow/Unfollow

  - **Follow on Qobuz** — subscribe to any playlist directly on your Qobuz account; syncs across all Qobuz clients
  - **Unfollow** — remove subscription from Qobuz
  - **Favorites vs Following** — new sub-tabs in Favorites > Playlists to separate local favorites from Qobuz subscriptions
  - **Bookmark toggle** — dedicated button in playlist detail view alongside existing Copy and Favorite actions

---

## Audio Settings Sync Overhaul

A critical fix for exclusive mode and bit-perfect playback: audio settings were not being propagated to the CoreBridge player when changed through the UI.

  - **All critical setters now sync immediately** — backend_type, exclusive_mode, dac_passthrough, output_device, alsa_plugin, pw_force_bitperfect, sample_rate, and alsa_hardware_volume
  - **Session activation sync** — per-user audio settings are pushed to the CoreBridge player on login, not just at startup
  - **alsa_hardware_volume fix** — was hardcoded to `false` in the conversion function; now reads the actual setting
  - **v2_reset_audio_settings** — now syncs defaults to the player after reset

## Volume Lock for ALSA Direct hw:

  - **100% volume lock** — when ALSA Direct with hw: plugin is active, software volume is locked at 100% (volume must be controlled at the DAC/hardware level)
  - **Visual lock** — volume slider disabled and grayed out with tooltip explaining why
  - **Backend guard** — v2_set_volume forces 1.0 regardless of frontend requests when ALSA Direct hw: is active

## PipeWire Rate Switching

  - **Longer rate switch delay** — 300ms → 500ms for USB hubs/docks that need more time
  - **Rate verification** — after stream creation, queries `pw-metadata clock.rate` to verify PipeWire actually applied the requested rate
  - **Automatic retry** — if rate mismatch detected, retries with additional 500ms delay and re-forces the rate
  - **Logging** — warnings when rate can't be verified so users can report issues

## HiFi Wizard

  - **Real DAC sample rates** — now reads actual discrete rates from `/proc/asound/cardN/stream0` instead of hardcoded defaults; fixes incorrect 176.4kHz showing for DACs that don't support it
  - **Help tooltips** — `(?)` icons on Exclusive Mode, DAC Passthrough, and Force Bit-Perfect settings with detailed explanations of what each does, which backend it requires, and how they relate

## Booklet Viewer

  - **MuPDF 0.6** — upgraded from 0.5; fixes struct size mismatch on Arch Linux
  - **Always available** — removed the `booklet` feature gate; MuPDF is now a direct dependency (matching v1.1.19 behavior)

---

## Offline Mode

  - **Network error screen** — "Failed to connect" error box now shows a "Start Offline" button alongside Retry, matching the timeout and login form behavior

## Window Management

  - **KWin SSD hack removed** — the KWin window rule approach (writing to kwinrulesrc) caused double titlebar and CPU spikes on KDE Plasma 6; removed entirely
  - **Stale KWin rules cleaned** — existing rules from v1.1.14-v1.1.20 are automatically removed on startup
  - **Window size clamping fixed** — previous approach used xdpyinfo which returns combined multi-monitor resolution, causing false positives; now only guards against obviously corrupt values (>8K), falling back to 1920x1080

## Playback

  - **Error messages** — playback failures now show actual error details instead of `[object Object]`
  - **Souvlaki fix** — MediaControlError formatting uses `{:?}` instead of `{}` for cross-platform compatibility

## UI Improvements

  - **Artist album counts removed** — Qobuz API's `albums_count` includes compilations, tributes, and appearances; removed from Favorites and Search to avoid misleading numbers
  - **Documentation link** — added to sidebar gear menu, opens the GitHub wiki
  - **MusicBrainz privacy note** — help text now clarifies that QBZ has no telemetry; MusicBrainz is a one-way pull
  - **Linebed viewport** — canvas offset to avoid overflowing into player controls area

## Distribution

  - **Snap MPRIS removed** — dropped mpris slot from snapcraft to unblock Snap Store review
  - **Issue templates** — migrated from markdown to YAML forms with dropdowns and required fields
  - **Dependency updates** — devalue 5.6.4 (prototype pollution fix), SvelteKit 2.53.3 (deserialization DoS fix)
  - **Nix build** — credential roundtrip tests now skip correctly in Nix sandbox

## Bug Fixes

  - Portuguese locale added with correct display name
  - Artwork proxy through backend cache for track rows and album headers
  - Local library disc/CD suffix no longer misidentified as disc folders
  - Discover menu items navigate to correct home tab
  - CDN premature EOF handled with fresh URL retry and quality fallback
  - ALSA device lock prevented on streaming failure
  - Streaming playback with seekbar position tracking
  - Session restore hardened to prevent ghost CSS errors

---

Special thanks to **@afonsojramos** for the souvlaki cross-platform fix (from PR #181).

Full changelog: https://github.com/vicrodh/qbz/compare/v1.1.20...v1.2.0
