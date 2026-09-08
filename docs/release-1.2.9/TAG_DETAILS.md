# 1.2.9 — Quiet Polish 

Not big features on this release, just polishing, bugfixing and
performance related changes.

A handful of smaller fixes and conveniences round it out: sticky
headers no longer leave a gap at the titlebar, an Edit folder
shortcut finally lives in the playlist sidebar context menu, and
shift-range + Ctrl/Cmd+A multi-select now reaches the remaining
views.

Two main threads of work since 1.2.8: splitting the single-file
QConnect service into a proper module so the History duplicates and
missing-tracks regression could finally be untangled, and a long
overdue round of tray polish on Linux. Details on each below.

Much of the work in this version is focused on laying the groundwork 
for the CLI and daemonized versions. The next release will likely take 
a little longer to arrive —unless there’s a major bug to fix, which 
I hope isn’t the case— because the work on the daemon is turning out 
to be much more involved than I expected. 

I mean, it’s straightforward, but we want a daemon that’s on par of 
quiality with the current client, right? 


---

## UI polish

  - **Sticky headers pin flush** — the gap that opened at the titlebar was caused by `padding-top` on the scroll container shifting the scrollport's inset edge; padding moved onto the first child instead
  - **Edit folder from the sidebar** (#364) — playlist-folder right-click menu opens the folder editor; pencil button mirrored on the Playlist Manager breadcrumb
  - **Multi-select** — shift-click range + Ctrl/Cmd+A across the remaining views; `BulkActionBar` floats above the player and docks at list end via IntersectionObserver
  - **Stop-after / remove-after** — queue context menu gets a one-shot stop marker and a tail-drop from a pivot
  - **Stratego theme** — imperial crimson + bone white with AAA contrast on every text level

---

## Streaming & playback

  - **Track version** (#360) — Qobuz's subtitle/edition flows through queue builders, MPRIS, immersive views, and the Last.fm + ListenBrainz scrobble payload
  - **Session resume position** (#317) — opt-in restore of the seek position on next launch
  - **Gapless one-shot guard** — prefetch-attempted bit now gates the gapless transition so a stray late-cancel can't trip the next-track hand-off
  - **Audio enumeration** — virtual ALSA PCMs are skipped during the CPAL probe; libasound's verbose enumeration errors route to `log::debug`
---

## QConnect (#316)

  - **Service split into a module** — `qconnect_service.rs` is now twelve focused files (transport, session, queue resolution, CoreBridge bridging, event sink, track loading, types, commands, tests…)
  - **History duplicates + gaps fixed** — cursor-align skipped when local is the active renderer; `set_queue` remaps history by track id instead of clearing it on every echoed reorder
  - **First-track hiccup, prev/next bouncing, shuffle drift** — single pass through the cursor-resolution path

---

## Look 'n Feel

  - **Cache images on sidebar** - Collages on the sidebar are now cached for a smoth scrolling. 
  - **Live tooltip** — track title / "by Artist" / album on hover, plus inline hints (Middle-click to pause, Scroll to adjust volume) that flip with state
  - **Middle-click + scroll wired** — middle-click toggles play/pause and vertical scroll adjusts volume in 5 % steps, mirroring the Plasma media plasmoid
  - **Icon variant picker** — Auto / Mono light / Mono dark / Color dropdown in Settings → Appearance → System Tray, for desktops where auto-detection picks the wrong glyph (e.g. GNOME's permanently dark top bar)
  - **Updates from a dedicated thread** — ksni 0.3's blocking handle panics from a tokio context; the new worker thread sidesteps that entirely

---

## Internal

  - **Legacy V2 wrappers removed** — library, network, offline_cache, reco, cast/dlna and api_server playback no longer carry placeholder commands that just delegated to V2
  - **Mixtape shuffle internals** — `shuffle` module with normalize + token_set_ratio, `dedup_by_similarity`, `hybrid_sample` with per-album cap; new `v2_collection_unique_track_count` / `v2_collection_shuffle_tracks` commands
  - **x86 baseline relaxed** — release builds drop the x86-64-v3 baseline so pre-Haswell CPUs no longer SIGILL on launch
  - **Dependency bumps** — typescript 6.0.3, ashpd 0.13.10, notify-rust 4.16.0, rustls-webpki 0.103.13, plus the usual minor/patch swarm

---

Thanks for everyon collaborating with this project. 

This release also include contributions from @afonsojramos as usual, thank your for yor commitment with this and the MacOs version of Qbz. 

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.8...v1.2.9
