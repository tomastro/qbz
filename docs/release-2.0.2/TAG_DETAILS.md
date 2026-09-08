# 2.0.2 — Rebuild 破 (You Can (Not) Advance)

Oops, this release grew bigger than expected! Version 2.0.2 started as a maintenance patch but ended up pulling in several pending features that made perfect sense after the switch to Slint. Here are the main highlights:

The QBZ Daemon: A highly requested, headless binary. While still an early version, it has the essentials to hook up your DAC to a Raspberry Pi and run QBZ directly from the terminal, fully supporting Qobuz Connect.

Kiosk Mode: A simplified, touch-friendly UI—admittedly a personal treat for my own Raspberry Pi and HiFi hat setup.

Desktop Improvements: A new dynamic album art background, heavy audio chain hardening, and better overall performance.

---

## The headless daemon (qbzd)

The big one. `qbzd` is a single slint-free binary — the daemon, its own CLI, and an interactive setup TUI, all in one — meant for a box that's always on and wired to your DAC.

  - **A player with no window** — plays music, shows up in the official Qobuz apps as a castable Connect device, and configures itself over SSH; no desktop, no webview.
  - **Bit-perfect, same as the desktop** — it runs the exact protected audio core (ALSA direct, no forced resampling), so 192 kHz/24-bit stays 192 kHz/24-bit.
  - **A full remote control from the terminal** — search, play anything (a Qobuz id, a share URL, even a Deezer link), browse albums and artists, discover, radio, recommendations, favorites, playlist create/edit, queue editing, lyrics, cover art, shuffle and repeat — all as `qbzd <verb>`, local or across the LAN.
  - **Setup TUI** — a raspi-config-style configurator for account, audio/DAC, playback, Qobuz Connect and network, with the HiFi Wizard ported in, all over SSH.
  - **Scrobbling** — connect Last.fm and ListenBrainz from the terminal or the setup TUI; it scrobbles what it plays and queues listens offline so none are lost.
  - **Desktop media controls** — publishes MPRIS, so a KDE/GNOME media widget, a plasmoid, or your keyboard's media keys drive the daemon with no extra client.
  - **Live events** — `qbzd watch` streams track changes, queue edits and volume as newline-delimited JSON, ready to pipe into a status bar or a panel.
  - **Service files for any init** — `qbzd service` generates a ready-to-install systemd, OpenRC or runit unit and resolves the audio environment for you.

Full setup and usage instructions live in the daemon manual:

[Read the Headless Daemon guide →](https://github.com/vicrodh/qbz/wiki/Headless-Daemon)

---

## Library, home & discovery

  - **Library "All"** — a mixed feed with track cards, ownership-aware playlist cards, artist playlists, local favorites, and genre/sort controls. Just like the new "All" from the official app, but with Local library support (#320). 
  - **Pinned on Home** — a per-user Pinned section as a mixed carousel with pin affordances; recently-played rails auto-refresh. Chose what you want to see first, pin your current obsession or all time favorites in your Home or For You sections. 
  - **Playlist reorder** — drag-and-drop for custom-order tracks using the shared drag gesture, plus an optimistic sidebar rename. Fixing some regresion of lost features from the Tauri version. 

---

## Kiosk mode

At the opposite end of "no window at all," a touch-first face for touchscreens and small panels.

  - **Opt-in profile** — set `QBZ_PROFILE=kiosk` for a big-target, touch-friendly interface built for a Raspberry Pi screen or a small display, I've been testing on that exactly and in a Steam Deck and I'm so happy with the results, hope you like it too.
  - **Its own shell** — a NavRail, touch scrolling, an on-screen keyboard, and lightweight Search / Library / Discover / Album / Artist views that only build what's on screen.
  - **A centerpiece Now Playing** — a dominant cover, a cover↔lyrics toggle with synced follow, and queue/history tabs.
  - **Switch on the fly** — a live Kiosk↔Desktop toggle in the Now Playing layout menu; boots windowed by default, fullscreen opt-in via `QBZ_KIOSK_FULLSCREEN`.

---

## A dynamic background

This feature is a personal indulgence stemming from my stubborn belief that we can indeed have a good-looking music player on Linux. It requires a dedicated GPU and uses up resources, so use it if you have no problem making that nice, idle GPU do a little work. (Not that it's going to heat up your room.)

  - **App-wide album-art background** — turn on a backdrop that blooms behind the whole shell, with ambient shaders and translucent bars, panels and controls.
  - **Shows through the content** — the backdrop reads through the content area, with glass carousels and a livelier ambient scene in immersive mode.
  - **Per-GPU selector** — choose the adapter that drives it. This feeature will put your GPU to work, but with care of course. 

---

## Hardening, fixes & polish

The rest is the kind of work you feel more than see — the full list is in the changelog.

  - **Audio & playback** — ALSA-exclusive and DAC fixes (#641, #508), fail-closed on inexact exclusive rates, sturdier player / DSD / CMAF paths, and a fully fluid (de-quantized) volume slider.
  - **Rendering & performance** — femtovg partial rendering so only what changed repaints (#617), one wgpu device reused across Wayland surface recreations (#558), and smarter weak-GPU tiering.
  - **New Catppuccin themes** — Latte, Frappé and Macchiato, thanks to [@TerminalTilt](https://github.com/TerminalTilt); plus a searchable theme dropdown and light-theme legibility fixes.
  - **Interface** — a unified track context menu, cursor-anchored card menus, and assorted layout fixes.
  - **Tighter security** — secret files stored `0600`, redacted logs, and Last.fm token prefixes no longer logged.
  - **Casting with guardails** — If your streamer/receiver doesn't support HiFi or strugle with certain high sample rates, this one is the one feature that you are looking for. 
  - **Under the hood** — a headless playback driver, a settings bundle engine, a Qobuz 403 circuit breaker, and routine dependency bumps.

---


## What's Next

Local Server Integration: Jellyfin and Navidrome support are up next.

Headless Improvements: Hardening the daemon and gathering community feedback for future iterations.

Instance-to-Instance Remote Control: Running parallel to Qobuz Connect, this will let you control one QBZ instance from another (e.g., controlling your bathroom's Raspberry Pi from your office desktop).

---

## What Won't Be Developed

  - A TUI Player or WebPlayer: The daemon covers most use cases and its documentation and design makes it easy to build custom controls with minimal effort—whether you want a Plasma widget or a simple web frontend, in example. 
  If you are looking for a strictly CLI experience, I recommend checking out [Qobit](https://github.com/pierdom/qobit) (bit perfect is based on QBZ, thanks [@pierdom](https://github.com/pierdom/) for the attribution), [Hifi.rs](https://github.com/iamdb/hifi.rs), or its fork, [Qobine](https://github.com/SofusA/qobine).
  
Full changelog: https://github.com/vicrodh/qbz/compare/v2.0.1...v2.0.2
