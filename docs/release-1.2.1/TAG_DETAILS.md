# 1.2.1 — Resilience

A stability release focused on **resilience against Qobuz server issues**, with a completely rewritten retry system, and a wave of fixes across offline mode, graphics compositing, and QConnect.

---

## Exponential Backoff and Quality Fallback

Completely rewritten retry logic for audio downloads. When Qobuz CDN returns errors (504, 502, 503), QBZ now retries with exponential backoff instead of giving up quickly.

  - **4 retry attempts** with 1s/2s/4s exponential backoff (was 3 attempts with fixed 500ms)
  - **Quality fallback** — automatically tries the next lower quality tier on failure, then retries at that tier
  - **User preference modal** — after all retries fail, asks the user to try lowest quality or skip. Choice can be persisted ("Remember my choice")
  - **Settings dropdown** — "When quality retries fail" option in Audio Settings (Ask me / Always try lowest / Always skip)
  - **Prefetch benefits** — same backoff logic applies to background cache pre-loading
  - **Persistence** — user preference protected by ADR-003, survives audio settings reset

## Degraded Service Indicator

  - **Orange warning triangle** in the player bar when Qobuz servers are returning repeated errors
  - **Auto-clears** after 5 minutes without new server errors
  - **Settings notice** — degraded status also shown in the offline settings section

## Manual Network Check

  - **Clickable offline indicator** — the yellow offline icon in the player bar is now a button that forces an immediate network check
  - **Check Now button** in Settings > Offline section

## Offline Mode Improvements

  - Restore offline mode using last known user profile when Qobuz is unreachable
  - Initialize all user stores during offline session activation
  - Detect BundleExtractionFailed in catch block for offline mode
  - Dismissible offline notice banner in Local Library
  - Allow network folders in manual offline mode
  - Fix logout spinner hang and last_user_id preservation

## Graphics and Compositing

  - Prevent image flicker on WebKitGTK 2.50+ (compositing layer fix)
  - Fix hybrid Intel+NVIDIA treated as NVIDIA-only for compositing detection
  - Respect saved graphics settings at startup
  - System diagnostics panel in Developer Mode (on-request, not always visible)

## QConnect Fixes

  - Tolerate missing queue_version major/minor fields
  - Refresh session snapshot when badge popup opens
  - Allow volume control when peer renderer is active on ALSA hw: devices

## Other Improvements

  - Proxy all images through reqwest to bypass WebKit TLS issues (#163)
  - ESC in immersive mode now exits fullscreen too (#202)
  - SystemTooltip wraps long text instead of overflowing (#166)
  - Retry download with effective quality after fallback (#196)
  - HiFi Wizard: non-systemd support and verify step fix
  - Batch download status checks on Home view (performance)
  - Platform dropdown in GitHub issue templates (Linux/macOS) with auto-triage
  - Contributors: added afonsojramos (macOS) and GwendalBeaumont (i18n)
  - i18n: hardcoded strings replaced in SearchView, PlaylistDetailView, AlbumMenu

## macOS Port (Foundation)

QBZ is a Linux-first application. Initial macOS support is experimental and limited. Thanks to [@afonsojramos](https://github.com/afonsojramos) for driving this effort.

  - Gate Linux-only audio backends, commands, and network modules with `cfg(target_os)`
  - Native title bar adaptation for macOS
  - SystemDefault audio backend variant
  - macOS CI workflow (unsigned DMG, aarch64)
  - notify-rust for desktop notifications on macOS
  - 16-bit PNG to 8-bit conversion for Tauri icon compatibility

## Distribution

  - Gentoo overlay workflow for auto-update on release
  - Snap build-packages: added libclang-dev and unzip for mupdf-sys
  - Flatpak tray icon path fix for sandboxed environments

## Security

  - rustls-webpki 0.103.9 -> 0.103.10 (CRL Distribution Point fix)
  - aws-lc-sys 0.38.0 -> 0.39.0 (X.509 Name Constraints Bypass, CRL Scope Check fix)

---

Thanks to [@GwendalBeaumont](https://github.com/GwendalBeaumont) for the i18n contributions and [@afonsojramos](https://github.com/afonsojramos) for driving the macOS port.
