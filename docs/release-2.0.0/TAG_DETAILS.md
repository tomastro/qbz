# 2.0.0 — Rebuild 破 (You Can (Not) Advance)

It's been a while since the last release — in software terms that's not really that long, it's actually pretty normal, but it did break the weekly-release rhythm we had going.

It's been something like a month and a half of hard work — more than the initial launch, honestly — but I think it was worth it. I've dared to mark this as a major version bump: this is qbz 2.0.

QBZ was originally supposed to be a small project. I leaned hard into vibe coding, for better, but I also made some decisions based more on hype than on the future, and that caught up with me fast. I built it on Tauri, which I still think is an amazing tool — it brings the ease of web development to desktop apps, not as a WebApp like Electron, but it still depends on web engines underneath. Tauri is perfectly configurable to run well on any machine, but at some point I had a graphics settings section bigger than the audio settings section — absurd for an audio player.

That felt unacceptable, so I started a rebuild I'd basically had in mind since week 1 of QBZ's life. Since the 1.2.x releases I'd been quietly making backend changes to enable this migration. Well, here it is.

---

## The rewrite

  - **Frontend completely rebuilt** — considerably smaller CPU and memory footprint; memory now goes to playback, not to drawing the app.
  - **Wayland/X11/Metal, all the same** (Direct3D tested... spoiler alert?) — no more chasing the right configuration, the app handles it for you.
  - **macOS is no longer second-class** — thanks to [@afonsojramos](https://github.com/afonsojramos) and [@Vudgekek](https://github.com/Vudgekek), the macOS port is now solid and out of its experimental phase; QBZ is a proper Linux and Mac platform.
  - **Now also in Russian and Japanese** — the UI ships in 7 languages; thanks to [@mxnix](https://github.com/mxnix) for the Russian translation.
  - **Friendlier, more minimalist layouts** — focus on content and music instead of piling options on screen (try small mode with the sidebar fully closed, it's my favorite).

---

## Audio, playback & Qobuz Connect

  - **Playback engine hardening** — more than one problem fixed here; the engine is considerably more solid now (Pulse out of the box, Jack available).
  - **HiFi Wizard revamped** — one of QBZ's crown jewels got hardware auto-detection and simpler tests, without being intrusive; configuration is still yours to make, just easier to get there.
  - **Qobuz Connect, more stable** — still work to do for full 1:1 parity with the official clients, but noticeably closer.

---

## Search, lyrics & blocking

  - **New search overlay** — makes results more accessible, backed by a small cache layer that learns your preferences the more you use it, so it stops surfacing results you never touch.
  - **Faster lyrics** — now pulled straight from Qobuz since they offer them; the fallback to our previous lyrics engine is still there for when Qobuz doesn't have them.
  - **Granular blacklist** — block artists or albums individually, not just genres — goodbye to that reggaeton you don't like, that covers album, the AI slop, or the same-name artist sneaking into your results. Fully reversible and manageable.

---

## Offline & local music

  - **Login-free local music** — use QBZ for your local library without ever logging into Qobuz.
  - **Fully offline playlists** — create and manage playlists with zero Qobuz connection; offline mode overall is just more practical now.
  - **DSD file support** — because what's a HiFi player without DSD?

---

## Discovery & recommendations

  - **New recommendations tab** — Last.fm and ListenBrainz/MusicBrainz were already wired in for scrobbling but underused; connect them and get recommendations based on what you listen to, similarities, and local-listen vectorization. Logic, not AI. If you haven't connected these yet, do it.
  - **Better in-playlist recommendations** — improved vectors, disambiguation, and more.

---

## Immersive mode

  - **Trimmed, but for the better** — a couple of views were dropped, others added, and the ones that remain are no longer a resource drain; more and better full-screen views are coming.
  - **Search overlay now works inside immersive mode** — switch albums without leaving the view. The goal is something like the legendary Winamp and Windows Media Player visualizers of the 2000s — the ceiling will be an Xbox 360-style visualizer, haha.

---

There's a lot more under the hood that you probably won't notice visually, but you will in day-to-day use. I hope you enjoy this new version as much as I enjoyed working on it.

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.15...v2.0.0
