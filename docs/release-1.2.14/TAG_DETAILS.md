# 1.2.14 — Just cleaning

No new features this time. 1.2.14 is a maintenance release: the headline is that audio now **just works out of the box** for everyone, including people who never touched an audio setting.

The new **System** output plays through your system's default device and mixes with other apps like any normal player — no DAC, no bit-perfect, no `pactl` required. Audio focused users keep their explicit PipeWire and ALSA Direct setups exactly as they were.

If you have a DAC or you've already touched your audio settings, nothing changes for you. I focused all the development effort on bit-perfect from the very first release and completely forgot about everyone who just wants to play music without caring about quality, bit depth, etc. Sorry for the small-world mindset.

The rest is a steady round of bug fixes, a coordinated audio-dependency update, and groundwork under the hood.

---

## Audio

  - **System output (new default)** — shared system default that works out of the box and mixes with other apps. No bit-perfect, no `pactl` — ideal if you just want it to play.
  - **Auto picks the best backend** — selecting Auto resolves to the best available backend (PipeWire when present, otherwise System) and shows what it chose, instead of leaving a silent, frozen state.
  - **PipeWire without pactl** — the backend is detected via its runtime socket, so it is no longer hidden on minimal Debian/Ubuntu installs (#466).
  - **Stream rebuild on rate change** — the output stream is rebuilt on a sample-rate change for all backends (#449).

---

## Fixes

  - **Album release date** — the hover overlay shows the full date again ("MMM D, YYYY"), not just the year (#469).
  - **Queue drag-and-drop on macOS** — reordering by drag is restored (#453).
  - **Favorites click-to-play** — the remix/edition version is carried on the first click from the favorites view (#452).
  - **Discovery favorite button** — wired up on the browse / see-all pages (#468).
  - **Gapless & offline** — the gapless target is never skipped offline, the skip cascade is bounded, transient fetches retry with backoff (#467).

---

## Under the hood

  - **Audio dependencies** — cpal 0.17.3 and alsa 0.11, for smoother ALSA device enumeration and stream teardown.
  - **Log hygiene** — hardened redaction of sensitive data in navigation logs.
  - **App-core groundwork** — playback context, session store, shell facade, and a shared image cache + credential store moved into framework-agnostic crates, with no change in behavior.
  - **Dependency bumps** — the usual dependabot round.

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.13...v1.2.14
