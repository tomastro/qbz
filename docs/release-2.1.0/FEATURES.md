# 2.1.0 — Feature inventory 

## New platforms

- **Windows support** — QBZ now ships a native Windows build (MSI installer):
  same UI, same Rust core, WASAPI exclusive mode with rate probing, media
  keys, deep links, single instance. This is deployed with a disclaimer: **Maintainer wanted**. 

## Purchases

>Purchases has been integrated—not from the very beginning, but since one of the early versions of QBZ. Its presence in the menu is **opt-in** (So, you need to enable it on Settings > Appeareance), because some users don’t buy albums and only use streaming. Strictly speaking, purchases are limited by region; in Mexico, for example, they’re disabled. I couldn’t buy anything, so all development in that area had been done “blind.” A user helped me, but it was very complicated; certainly, rolling out a feature I couldn’t verify was something I don’t plan to do again. 
During my vacation, I was able to create an account without the region lock, and this opened the door to properly implementing a feature: 

- **Download What You Bought**  — The previous method was completely wrong; it downloaded files that were supposed to be for streaming. Now, the files served with the intention of being downloaded are downloaded—that is, exactly the file you purchased. 
- **Downloads in the purchased quality levels**  — From MP3 to DSD, you can download your DRM-free purchases from QBZ and save them wherever you want, all from the same app.
- **Download history and direct integration with your local library**  — You can now keep track of what you’ve already downloaded if you’ve added it to your local library.
- **Integration with the Albums page**  — Purchases have been integrated with the Albums view; you can choose to play either the streaming version or the version you purchased. If you’ve already bought a DSD file, why listen to a FLAC? 

## Playlists & queue

- **Add to playlist, redesigned** (revamp) — the picker shows which playlists
  already contain the track ("Already in"), pins your last-used playlist,
  handles multi-track adds, and accepts local and media-server tracks.
- **Album Quick View** (revamp) — peek into any album straight from its card,
  wherever you are, without leaving the page.
- **Play later, for real** — album-level "later" now queues after the current
  block instead of appending at the end; "Play all later" on artist pages.
- **Extended queue view** — the queue got a full-page view, drag-and-drop
  insert at position, reorderable history, and playback history that
  survives restarts.
- **Playlist importer expansion** (revamp) — import from playlist files,
  JSON, ListenBrainz and Last.fm.

## Qobuz Connect

- **LAN pairing like the official apps** — QBZ announces itself on your local
  network so official Qobuz clients can discover and pair with it directly.
- **Connect hardening** (revamp) — shuffle order, volume, buffering states
  and handoffs now match official-client behavior exactly, verified against
  the iOS app and the Web Player.

## Casting

- **Casting round two** (revamp) — progressive serving, clean shutdown, and
  Plex/Jellyfin/Subsonic tracks cast through a local proxy.
- **Visualizers while casting** — a shadow decoder keeps the scopes and
  spectrum alive while audio renders on the cast device.
- **Older Chromecasts fixed** — devices with X.509 v1 certificates can cast
  again (#730).


## New sources & library

- **Jellyfin integration** — browse, search and play your Jellyfin server as
  a first-class source, verified bit-perfect against a real server.
- **Subsonic integration** — same treatment for Subsonic-compatible servers
  (Navidrome, etc.), also verified bit-perfect.
- **Library Explorer** — a new tree-style browser over your whole collection,
  every source in one place, with drag-and-drop into playlists and the queue.
- **Unified local catalog** — the Local Library is now backed by a real
  catalog: incremental scans and server syncs, smooth paging with tens of
  thousands of tracks, album versions unified across sources.
- **Local tag editor, expanded** (revamp) — bigger editing workspace, safer
  writes to file, remote metadata lookup.

## Discs

- **Audio CD playback** — insert a CD and play it, on Linux and macOS, with
  metadata lookup and cover art.
- **SACD image playback** — open a SACD ISO and play its stereo program
  directly.
- **CD ripping** — a rip wizard that writes tagged, bit-exact, seekable FLAC,
  with metadata correction, covers and a provenance log; QBZ remembers the
  disc after eject.

## Audio & playback

- **ALSA hardware volume** — opt-in control of your DAC's own hardware mixer,
  detected by real capability (never by guessing names); asks when ambiguous,
  fails safe when it can't be sure.
- **Playback recovery** — a dead stream or unreachable track can no longer
  wedge the queue: QBZ retries with deadlines, then moves on.
- **Stereo scopes** — a new visualizer drawing both channels, oscilloscope
  style.
- **Listening history (listen log)** — a private, local log of what you
  actually listened to, with its own toggle and clear; the foundation for
  offline recommendations.
  

## qbzd (headless daemon)

- **Event hooks** — `qbzd settings set hooks.script /path/to/script` (or
  `QBZD_HOOK`) runs your executable on every daemon event with `QBZ_*` env
  variables — the same push contract as pleezer and shairport-sync, made for
  moOde/Volumio/DIY audio boxes (PR #700, Filippo Vicentini).
- **Gapless robustness on constrained hardware** — successors warm on every
  track start and cold hand-offs stream instead of buffering, closing the
  intermittent-gapless report on a 1 GB Pi (#699); verified on real hardware.
- **ALSA hardware volume in the CLI/TUI** — same capability probe and closed
  selection as the desktop app.
- **Standalone packaging & appliance use** — qbzd ships as its own package,
  masks IPs in logs, and read-only/appliance deployment is documented.
- **State reliability** — hardened QConnect ownership lifecycle and quality
  caps, deduplicated bridge state changes, correct TrackStarted on replays.

## UI & experience

- **Immersive, expanded** (revamp) — more shader scenes, a split view with
  lyrics/queue/track info, album-palette accents, karaoke line dimming, and
  light themes finally legible over the ambient background.
- **Vim keymap** — the hotkey system grew a Vim mode (PR #724).
- **Sponsors in About** — GitHub and Ko-fi supporters now listed in the app.

## Community fixes worth naming (pre-2.1 issues)

- Real desktop keyring backends for credentials (#697) and a mobile KDF
  fallback (#695).
- Relocatable offline cache bundles (#696) with a configurable cache
  directory (#708) and atomic cache replacement (#707).
- MPRIS position is exact at read time — external widgets stop drifting.
