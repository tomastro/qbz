# 1.2.3 — Hardening playback

Qobuz has been migrating their audio delivery infrastructure from direct FLAC downloads to encrypted, segmented streaming via their Akamai CDN. This was silently breaking playback for certain tracks — some albums would fail after 1 byte with no clear error. This release implements the new CMAF streaming pipeline end-to-end, making QBZ compatible with Qobuz's current and future audio delivery.

Beyond the streaming overhaul, this release hardens every part of the playback chain: faster track skipping on ALSA, smarter quality negotiation with the API, and fixes for several edge cases that caused dropped tracks or silent failures.

---

## Streaming

  - **CMAF segmented streaming** — new `qbz-cmaf` crate implements Qobuz's encrypted CMAF pipeline: HKDF-SHA256 session key derivation, AES-128-CBC content key unwrapping, and AES-128-CTR frame decryption. Audio segments are fetched from Akamai CDN, decrypted on the fly, and reassembled into standard FLAC for playback and caching
  - **Streaming playback with immediate start** — only the init segment (~500ms) is fetched before playback begins; remaining segments download in background while the track plays
  - **CMAF-first with legacy fallback** — all download paths (play, prefetch, streaming fallback) try the new pipeline first and automatically fall back to the legacy direct download if CMAF is unavailable
  - **Smart quality negotiation** — the frontend now sends the track's actual quality (from metadata) instead of always requesting the maximum. A 44.1kHz/24-bit track requests HiRes directly (1 API call) instead of cascading through UltraHiRes restrictions (3 API calls). Cache matches are now exact, eliminating unnecessary re-downloads

## Audio

  - **ALSA progressive backoff on device busy** — when skipping tracks quickly, the ALSA backend now retries device acquisition with progressive delays (50/100/200/400ms) instead of a single 200ms attempt, preventing "device busy" playback failures (#270)
  - **alsa crate 0.9 to 0.10** — upgraded with dead dependency cleanup; the `alsa` dependency was removed from `src-tauri` (leftover from pre-V2 migration) and properly placed in `qbz-audio` only

## Playback Fixes

  - **Ghost next_track prevention** — added queue epoch counter to discard stale auto-advance commands that arrive after a context switch (album/playlist change), preventing skipped tracks (#270)
  - **Offline cache connection pool fix** — `StreamFetcher` now creates a fresh HTTP client per download with retry logic (3 attempts, exponential backoff), preventing HTTP/2 connection pool poisoning that caused persistent "1 byte then EOF" failures (#268)
  - **Logout stops playback** — logging out now explicitly stops audio and clears MPRIS state before tearing down the session
  - **Offline playback controls** — pause, resume, stop, seek, and volume commands now work without API client initialization, fixing playback control failure when the app starts without internet (#173)

## Security

  - **System keyring for OAuth tokens** — OAuth tokens are now stored in the system keyring (GNOME Keyring, KDE Wallet) when available, with the existing AES-256-GCM encrypted file as automatic fallback. Existing tokens migrate to the keyring on first load (#215)
  - **Flatpak keyring access** — added `org.freedesktop.secrets` to Flatpak finish-args for sandbox keyring support
  - **Vite 8.0.5** — fixes 3 dev server vulnerabilities: path traversal in optimized deps, arbitrary file read via WebSocket, and server.fs.deny bypass

## i18n

  - **60+ components translated** — massive i18n pass covering immersive mode, modals, menus, panels, and player controls across all 5 locales (en, es, de, fr, pt)

## Immersive Mode

  - **Larger artwork and typography** — increased cover art size, Montserrat font, +4pt font sizes in split panels
  - **Coverflow fixes** — restored center image layering, eliminated side cover overlap, added quality badge backdrop

## Other

  - **Platform detection via Tauri OS plugin** — replaces manual platform checks; Windows platform type added
  - **Dependency updates** — dirs 6, svelte 5.55.1, jsdom 29
  - **macOS** — ad-hoc signing for Gatekeeper, cross-platform settings visibility

---

Thanks to @afonsojramos and @GwendalBeaumont for their contributions to this release.

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.2...v1.2.3
