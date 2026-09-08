# 1.2.13 — 400+ stars

qbz just passed **400 stars on GitHub**. Thanks to everyone who has tried the player, filed an issue, or sent a patch — this release is named for that milestone.

There are no headline features this time. 1.2.13 is a polish and stability release: most of the work went into **macOS**, where @afonsojramos and @Vudgekek tightened audio playback, the title bar, and the updater.

The rest is a steady round of bug fixes, a new graphics auto-config helper for Linux, and groundwork under the hood.

---

## macOS

  - **Shared Mode playback** — fixed a speed drift, retries CoreAudio on a sample-rate mismatch, and rate-mismatch errors are now phrased for end users (@Vudgekek PR #410, @afonsojramos PR #420)
  - **Title bar & window dragging** — drag regions reworked across the sidebar and view top-bars; the legacy macOS overlay drag region is gone (@afonsojramos PR #423)
  - **Synced lyrics** — active-line color corrected under macOS WebKit
  - **Auto-updater** — `.app.tar.gz` bundles are suffixed with the architecture so the updater picks the matching build

---

## Session restore

  - **Full queue** — the entire play queue is saved on every change and restored on next launch, not just the current track (#315)
  - **Qobuz Connect priority** — when a local session and a Qobuz Connect session both exist at startup, the Qobuz Connect state takes over

---

## Linux graphics

  - **Graphics auto-config** — a new helper detects your GPU and recommends rendering settings; an advisory banner in Settings → Graphics shows what it suggests
  - **Boot watchdog** — if a risky graphics setting crashes the app before it paints, the next launch reverts it on its own
  - **GPU selection** — WebKit can be pinned to a specific GPU, which matters on hybrid NVIDIA + integrated laptops

---

## Fixes & polish

  - **Chromecast** — fixed casts that connected and chimed but never played; the keep-alive heartbeat now matches Google Cast timing (#439)
  - **Favorites on Qobuz tracks** — the favorite and add-to-playlist buttons are no longer wrongly disabled on streaming tracks
  - **Menus & dropdowns** — a click outside a menu always closes it, toolbar dropdowns no longer swallow that click, and opening one filter closes the others (@afonsojramos PR #421)
  - **Plex** — album format and bit-depth are recovered when the codec tag is missing
  - **Track lists** — unavailable tracks are dimmed consistently across every list view
  - **Local library** — optional album artwork in track and folder lists (#412), and smoother scrolling on large Albums grids

---

## Under the hood

  - **Dependency bumps** — Tauri 2.11, tokio, vite 8, and the usual dependabot round
  - **App-core groundwork** — playback, audio, and settings logic continues moving into framework-agnostic crates, with no change in behavior

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.12...v1.2.13
