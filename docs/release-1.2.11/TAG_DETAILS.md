# 1.2.11 — Exclusive Hardening

This release tightens bit-perfect playback on both fronts. macOS finally gets a real **CoreAudio Exclusive Mode** thanks to **@Vudgekek**'s first-class contribution (PR #391), and on Linux the player now actively **reserves the DAC** via the standard `org.freedesktop.ReserveDevice1` D-Bus protocol for the lifetime of an ALSA Direct stream. Both halves landed in the same cycle by happy coincidence — hence the codename.

Beyond audio, this is the largest **Local Library** release since the local engine landed: a new folder-tree mode, ephemeral playback that never writes to your library DB, and a metadata-grouped Albums tab that finally treats Plex as a first-class peer.

A new **Offline Cache Manager** view replaces the old Settings shortcut, with a configurable cache cap (5 GB default), live progress rings, and per-album expand. **Discord Rich Presence** lands as opt-in and works inside the Flathub sandbox via a small symlink bridge. macOS gets a proper title bar integration thanks to **@afonsojramos**, who continues to keep the macOS build healthy alongside this round of fixes.

Welcome to **@Vudgekek** and **@DoubleGate** as new contributors — thanks for the first PRs, and for taking the time to engage with the review process.

---

## Exclusive Audio

  - **macOS CoreAudio Exclusive Mode** (PR #391, @Vudgekek) — bit-perfect playback on Apple hardware via `kAudioDevicePropertyHogMode`; exclusive stream recreation on track changes; CoreAudio device-rate checked against the requested track rate before claiming the device
  - **macOS hardware volume alignment** — the exclusive device's hardware volume tracks qbz's volume slider when in exclusive mode (@Vudgekek)
  - **macOS exclusive stream hardening** — defensive cleanup on stream errors and shutdown so a failed claim never strands the device (@afonsojramos)
  - **Linux DAC reservation** — per-process zbus client acquires `org.freedesktop.ReserveDevice1` (the standard cross-app coordination protocol used by JACK, PipeWire, and PulseAudio) for the lifetime of an `AlsaDirectStream`; other apps see qbz as the sole DAC owner instead of getting silent, misleading "device busy" errors
  - **Reserve-DAC toggle** — Settings → Audio adds an opt-out for the reservation, persisted in `audio_settings.db` alongside the existing backend choice
  - **ALSA card resolution by id** — ALSA card matching now uses the stable card id (`hw:0`) instead of the long descriptive name; any plugin prefix is accepted and parse failures degrade gracefully
  - **`audio_settings` defaults** — `backend_type` now defaults to a per-OS choice and migrates rows where the column was NULL on older installs

---

## Local Library

  - **Folder Tree mode** — new two-column layout with the folder tree on the left and a folder detail pane on the right; drag-resizable sidebar (default ×1.8), recursive multi-select, search with path-parents preserved, and a network-folder filter for shares that take a while to walk
  - **Ephemeral folder playback** — pick a folder outside your watched library and play it without writing anything to the library DB, including CUE support, a format whitelist, ephemeral-aware gapless prefetch, and a session-restore filter so transient picks don't pin themselves
  - **Albums tab — metadata-grouped** — the existing folder-grouped Albums view is now joined by a metadata-grouped variant that merges entries across folders (and Plex), with the full action bar (group/filter/sort/view/select)
  - **Per-user Library tab order** — new modal to reorder, show, or hide tabs; persists per user; the active tab loader now triggers after preferences settle on mount, so the loader never hits the wrong tab
  - **Plex per-track artwork** preserved during library scan; the same fix is applied to both scan loops, with a folder-lookup cache to avoid the second-loop regression
  - **Compact album view** for the tree right pane, with selection + bulk actions, circular play, and a per-view track search
  - **Multi-select coverage in Album Detail** (#381) — bulk action bar now reaches the album-detail tracklist as well
  - **Local Library performance** — viewport-driven Plex hydration with a quality override map; batched track-quality with per-call timeout; sort pushed into SQL instead of `localeCompare` on the client

---

## Discord Rich Presence (#336)

  - **Opt-in track display** with album, artist, and elapsed time
  - **Native Discord** (deb / rpm / AUR builds) writes the IPC socket directly under `$XDG_RUNTIME_DIR` and works without setup
  - **Discord-via-Flathub** — qbz reads the Discord-Flatpak app dir (with `:create` permission) and lays down its own `discord-ipc-{0..9}` symlinks pointing at it; the symlinks bridge the two sandboxes for the `discord-rich-presence` crate, which only knows the static path
  - **Lazy bridge** — symlinks are only laid down after you enable the toggle in Settings, so users who never opt in never touch that filesystem path
  - **Translations** — keys for all five locales (en/es/de/fr/pt)

---

## Offline Cache Manager

  - **Dedicated view** replaces the old "Open folder" shortcut in Settings; navigate via the new sidebar entry or the Manage offline cache button
  - **Album rows** with expand-by-default, per-track actions, album cover artwork, and live progress rings on both album and track rows
  - **Alpha-index artist rail** — jump to artists alphabetically; empty-state CTA when nothing is cached yet
  - **Cache size limit** — modal to set a configurable cap (default bumped from 2 GB to 5 GB), persisted across restarts; new track downloads enforce the limit
  - **Re-download flow** — copy renamed from "Re-download album" to "Re-download all tracks" to match what actually happens
  - **Excluded from `last_view` persistence** — opening the manager doesn't pin it on next launch

---

## Sleep Timer (#402)

  - **Hard-stop pause** — at expiry the player pauses cleanly (no fadeouts that overrun the deadline)
  - **Smart-positioned popover** with refined typography; custom durations from 1 minute upward

---

## Title bar

  - **macOS integration** (@afonsojramos) — qbz custom titlebar now mounts on macOS with traffic-light reservation, drag band aligned to sidebar top padding; new nav and search titlebar toggles plus a hide-title-bar toggle in Settings; legacy macOS overlay drag region retired
  - **Mode selector** — Settings → Appearance → Title bar replaces the hide/system toggles with a `system / plasma / stripped` dropdown; legacy preferences are migrated, sandbox/Wayland caveats are surfaced as inline hints

---

## QConnect

  - **Startup persistence** — new `startup_mode` (Auto / Always-on / Disabled) and `last_known_state` write-through on connect/disconnect, both persisted; settings dropdown surfaces the choice with a local-library hint
  - **CLI override** — `--enable-qconnect` and `--disable-qconnect` flags applied during app setup
  - **Honor peer-controller seeks** when qbz is the active renderer — peer-driven seeks no longer get clobbered by the local cursor-align

---

## Album & Artist detail polish

  - **Header gradient backdrop** — palette-derived gradient and blurred artwork on Album/Artist detail
  - **Album metadata enrichment** — featured artist line, parental warning badge, click-to-expand description, tracklist toolbar, label/awards sidebar shrunk by ~20%
  - **Tracklist columns aligned** with smooth select-mode toggle

---

## Immersive background

  - **Kawarp WebGL renderer** replaces the legacy WebGL2 path in `Full` mode; attribution added to the About modal

---

## Bug fixes

  - **Mixtape detail** — virtualized list scroll-up behavior stabilized
  - **Library** — Albums tab visible in Edit Library Tabs modal; Plex albums included in metadata-grouped Albums view
  - **Audio (Linux)** — host stream drop deferred on Stop so the device can be reused across track changes (@afonsojramos)
  - **Audio (macOS)** — output device picker populated; the 50 ms post-stop sleep is now gated to `cfg(target_os = "linux")` (@afonsojramos)
  - **Playback** — uniform 250 ms state polling cadence eliminates seekbar cold-start latency
  - **Playlist** — track-selection key includes the row index for duplicate tracks (#386); multi-select is now aligned with the TrackRow checkbox pattern; the checkbox input is pointer-transparent so the row click handler always fires
  - **Sidebar** — `isOffline` `$effect` skips its initial subscription so a stale value doesn't flip the offline indicator on mount (@DoubleGate, PR #384)
  - **Home** — sticky tab header pop-in animation unified across the macOS and Linux paths (@afonsojramos, PR #407)
  - **Theme** — WCAG-aware `btn-primary-text` token across all themes (the previous fixed value failed contrast on a couple of light themes)

---

## Internal & packaging

  - **Node 20 → 24** across CI workflows and the Flatpak manifest
  - **Dependency bumps** — Tauri 2.10.3 → 2.11.1, openssl 0.10.79, tokio 1.52.1, axum 0.8.9, sha2 0.11.0, mdns-sd 0.19.1, plus the usual minor/patch swarm
  - **Flatpak permissions** — the manifest now declares `--filesystem=xdg-run/discord-ipc-0` and `--filesystem=xdg-run/app/com.discordapp.Discord:create` for the lazy Discord IPC bridge

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.10...v1.2.11
