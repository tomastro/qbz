# QBZ 1.2.14 — "Just cleaning" — Full changelog

Maintenance release. Out-of-the-box audio that just works, a smarter backend
picker, a coordinated audio-dependency update, a round of bug fixes, and
continued app-core groundwork. Explicit PipeWire (DAC passthrough) and ALSA
Direct (exclusive) configurations are unchanged and remain bit-perfect.

Compare: https://github.com/vicrodh/qbz/compare/v1.2.13...v1.2.14

---

## Audio

- **System output, the new out-of-the-box default (#470).** Added a cross-platform
  "System" backend that plays through the OS default output at its negotiated
  rate (rodio resamples), shared with other apps and without requiring `pactl`.
  It is the default for fresh installs and what "Reset Audio & Playback
  Settings" lands on. Previously the Linux default assumed PipeWire, which
  required `pactl` and froze playback (seekbar advancing, no audio) on hosts
  without it.
- **"Auto" is now a resolve-and-set action (#470).** Choosing Auto picks the best
  available backend — PipeWire if available, otherwise System — stores that
  concrete choice and updates the dropdown and device picker. The audio backend
  is never left in the ambiguous unset state that previously fell through to a
  fragile legacy path and could leave a stuck audio handle until restart.
- **PipeWire detected without pactl (#466).** The PipeWire backend is detected via
  its runtime socket, so it is no longer hidden on systems that do not ship
  `pactl` (pulseaudio-utils).
- **Output stream rebuilt on sample-rate change (#449).** All backends rebuild the
  output stream when the track sample rate changes.
- **Adaptive prefetch throttle.** Prefetch bandwidth is throttled based on observed
  throughput and underrun signals.

## Fixes

- **Album hover overlay shows the full release date (#469).** Restored the
  pre-AlbumCardLite "MMM D, YYYY" format in the discover/home card hover overlay
  (it had been reduced to the year only). Falls back to the year when only a
  year is available.
- **Queue drag-and-drop reordering on macOS (#453).**
- **Favorites edition/version on click-to-play (#452).** The remix/edition version
  is preserved on the initial click-to-play from the favorites tracks view.
- **Favorite button on discovery browse / see-all pages (#468).**
- **Gapless and offline robustness (#467).** The gapless target is no longer
  skipped when offline, the auto-skip cascade is bounded, transient fetch
  failures retry with backoff instead of skipping, and offline detection gains
  hysteresis plus a streaming-liveness override.

## Security / privacy

- **Hardened redaction of sensitive data in navigation logs.** Query-parameter
  values and URL fragments are redacted before reaching log output, keeping
  navigation traces useful without carrying values into log files or uploaded
  reports.

## Under the hood

- **Audio dependency update.** cpal 0.17.1 → 0.17.3 and alsa 0.10 → 0.11
  (alsa-sys 0.4), coordinated to satisfy the shared `links = "alsa"` constraint;
  smoother ALSA device enumeration and stream teardown.
- **App-core groundwork.** Shared image cache and credential store extracted into
  crates; minimal session activation, shell facade, playback context, and
  session store foundations added to `qbz-app`. No behavior change.
- **Dependency bumps.** @tauri-apps/cli, @sveltejs/kit, vite 8, vitest, @kawarp/core,
  open, tauri-build, tokio, ashpd, tower-http (dependabot round).
