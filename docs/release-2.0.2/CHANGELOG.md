# QBZ 2.0.2

A large maintenance release on top of the 2.0 native rebuild. Two brand-new
ways to run QBZ — a fully headless daemon and a touch-first Kiosk mode — plus a
new dynamic background, truthful casting/quality reporting, a Most Played
Albums rail fed by your own play counts, selectable album grouping in the local
library, and a long run of audio, rendering, library and security work.

## Headless daemon (qbzd) — new

A single slint-free binary that is the daemon, its own CLI client, and an
interactive setup TUI at once. Built for an always-on box wired to a DAC (a
Raspberry Pi, an LXC container, a NUC, a headless server).

- Runtime bring-up with daemon-root settings stores, credential restore, a
  `qbzd.toml` config (unknown-key warnings), a flock single-instance lock, and
  a `daemon_prefs` store.
- Browser-based login over SSH: a one-shot nonce-bound listener with LAN
  callback-host auto-detection from the SSH session, plus `--paste` and
  `--token` fallbacks.
- HTTP control plane (tiny_http) with an always-on Origin shield and an opt-in
  `[server]` bearer token; `ping`/`info`/`status` diagnostics.
- Full transport: `play`/`pause`/`toggle`/`stop`/`next`/`prev`/`seek`/`volume`/
  `mute`, running the complete advance ritual (skip → play → prefetch), with
  canonical JSON volume.
- Queue routes and verbs with server-side track materialization: `list`/`add`/
  `remove`/`clear`/`move`/`jump`/`stop-after`, plus `shuffle` and `repeat`.
- Qobuz Connect receiver: daemon device identity, volume modes (software /
  locked), report tick, and queue persistence.
- Setup TUI (ratatui): a raspi-config-style configurator — Account, Audio,
  Playback, QConnect, Network, Import/Export, the HiFi Wizard (copyable config
  blocks with honest OSC 52 semantics), and Scrobbler — with explicit-save
  (dirty-guard) semantics, a themed frames layout (sidebar nav, content frame,
  breadcrumb), and a responsive wide sidebar.
- Settings bundle hand-off: `settings export`/`import`/`show`/`set` with a
  masked-secret import summary and a `settings/reload` endpoint.
- Find & queue music from the terminal: `search`, `play <content>` (resolves a
  Qobuz id, a share URL, or a Deezer link), `album`, `artist`, `similar`,
  `suggest`, `discover`, `radio`, `reco playlist`, `lyrics`, `art`, `resolve`.
- Library from the terminal: `fav list/add/remove` and full playlist CRUD
  (`list`/`show`/`create`/`edit`/`rm`/`add`/`remove`).
- Scrobbling: connect Last.fm / ListenBrainz from the CLI (`scrobble login`)
  or the Scrobbler TUI screen; a scrobble-on-play engine keyed to the canonical
  scrobbler store, with a persistent ListenBrainz offline queue that drains on
  reconnect.
- MPRIS media controls: publishes `org.mpris.MediaPlayer2` (KDE/GNOME media
  widget, plasmoids, hardware media keys); TUI-toggleable (`playback.mpris`,
  default on), with a `QBZD_MPRIS` override.
- Live events: `GET /api/events` (Server-Sent Events) and `qbzd watch`
  (newline-delimited JSON by default, `--raw` for raw frames).
- `GET /api/artwork/current` — a stable 302 redirect to the current cover, for
  a dashboard `<img>`; `qbzd art` routes through it.
- `qbzd service [systemd|openrc|runit]` — generates a ready-to-install init
  service file, resolving the binary path and the target user's HOME /
  XDG_RUNTIME_DIR so audio and config roots resolve when it drops privileges.
- LAN-first posture: control plane and login bind `0.0.0.0` by default; a
  family-aware bind fallback; frozen exit-code taxonomy verified by a P0
  acceptance script.
- Spawn-and-ack on the load-bearing playback routes; daemon-originated queues
  published to the Connect cloud; `device_present` reported while the device
  is open; the persisted device name announced from every identity path.
- Daemon token key derived without the XDG portal secret; bounded CMAF
  fallback when the CDN header-floods a track.
- Bit-perfect: plays through the same protected `qbz-audio` / `qbz-player`
  core as the desktop — ALSA direct, no forced resampling.

## Kiosk mode — new

An opt-in touch-first interface for touchscreens and small panels
(`QBZ_PROFILE=kiosk`).

- KioskShell touch shell with a NavRail, forward navigation, window chrome,
  and a persistent search field in the back bar.
- Lightweight touch views that window their lists: Search, Library, Discover,
  Album, Artist, Local Library and MyQBZ; full ContentView route parity.
- Now Playing centerpiece — dominant contained cover, compact meta, a floating
  quality stamp, queue/history tabs, and a cover↔lyrics toggle with synced
  follow.
- Search "All" dashboard — a most-popular hero with per-type previews.
- Live Kiosk↔Desktop toggle in the Now Playing layout menu; NPB layout/lyrics/
  queue controls gated in kiosk.
- Boots windowed by default, fullscreen opt-in via `QBZ_KIOSK_FULLSCREEN`
  (`QBZ_KIOSK_NO_FULLSCREEN` dev escape hatch).
- On-screen keyboard dismissal on accept and tap-outside; touch-scrolling
  Settings via a raw Flickable + scrollbar; immersive-exit returns to the
  pre-immersive shell.
- Windowed album grid and list windowing so only visible cards mount.

## Dynamic background & shell

- App-wide dynamic album-art background shown behind the whole shell, reading
  through the content area.
- Metaball ambient shader with translucent bars, panels and controls; glass
  carousels and a livelier ambient scene in immersive mode; carousel fades
  dropped.
- Per-GPU selector in the shell.
- NVIDIA shader tiling fix; scrollbar / mixtape / track-listing batch.

## Casting & quality (#638)

- Quality badge reports the DELIVERED quality and names the downgrade cause
  (device cap, cast tier, source); a downgrade arrow with a true-quality
  tooltip on the Now Playing badge.
- Shared `QualityLimit` cause enum, `min_tier`, and a STREAMINFO prober in the
  models layer.
- Manual per-renderer quality cap: a cast-picker row, persisted in `ui_prefs`,
  applied to cast requests; device-cap copy mirrors the Settings tier names.
- Measured delivered quality reported on the cast surfaces; probed FLAC never
  labeled as MP3.
- Casting honors the streaming-quality preference; audio cache cleared when the
  streaming quality changes; prefetch at the cast-effective tier while casting.
- Local playback requests capped at the detected output-device limit; a
  truthful "limit quality to device" row with a read-only detected limit.
- F25 hydration values published with Release/Acquire; search-surface metadata
  hole closed.
- Chromecast discovery id stability exposed; residual DLNA transport state
  cleared on connect; seek by fraction of the cast track duration; CSPRNG
  media-path tokens and redacted URIs in logs, including the media-server
  request logs.
- DSD and hi-res PCM served to DLNA renderers (#646), with strict-renderer
  compatibility (HQPlayer6) and a self-healing Play after natural
  end-of-track.

## Audio & playback

- ALSA: aliased ids open as raw `hw:`/`plughw:` (#641); fail closed when the
  exclusive rate is not exact; #508 ALSA-exclusive + streaming regression
  fixed; Flatpak `ReserveDevice1` own-name; D-Bus device-reservation name
  failure treated as non-fatal; PipeWire no longer requires `pactl`; direct
  stop, drain and EBADFD write recovery unstuck.
- Continuous playback across sources: Plex tracks enqueued from MyQBZ
  collections resolve the numeric rating key instead of the album-boundary
  key — a Qobuz → Plex transition no longer 404s and leaves the previous
  track playing under the new track's UI.
- Playback-state tracking resumes when gapless lands late.
- DacCapabilities expose detected-vs-fallback and backfill a stale limit flag.
- Player: cancel superseded `play_track` completions; bump play generation on
  stop/DSD/streaming; clear streaming loaded state on setup failure; stop the
  DoP writer on ALSA write failure; clear playing state when a seek rebuild
  fails; surface stream-feeder failures to the buffer writer; superseded plays
  skip the 60s initial-buffer wait; legacy fallback on streaming-init failure.
- DSD: treat demux I/O errors as sticky failures, not clean EOF;
  `DsdErrorReport` holds `SharedState` directly.
- CMAF: fail init parse when raw_data exceeds the payload; fail closed on an
  invalid session-key seed hex.
- Volume: fully fluid (de-quantized) slider with live drag state; no rubberband
  on release.

## Rendering & performance (#617)

- femtovg partial rendering, stages 1–4 (scaffold through the full line), with
  cache-rendering hints on static shell chrome; partial-render blockers from an
  adversarial review addressed.
- One wgpu instance/device reused across Wayland surface recreations; the app
  stays alive when the wgpu surface goes Outdated (#558).
- Intel UHD 600 (Gemini Lake) classified as a weak GPU → GL tier.
- Layer/image caches invalidated on canvas reset; cache-rendering-hint removed
  from the Sidebar root.

## Library

- Selectable album-identity mode: group the Albums view by metadata or by
  folder — a compilation with mixed track artists renders as one card and
  multi-disc boxes merge — switched live from a header dropdown or Settings >
  Local Library, persisted, no rescan; untagged albums directly under a
  library root no longer inherit the root's name as artist.
- Scanner reads album / Album Artist / date across ALL of a file's tags
  (primary first), not just the primary one — repeatedly-retagged files with
  the album only in a secondary tag no longer render as folder-name albums
  with the oldest folder year (#447) or sort into Various Artists (#507).
- Single-album play-next / queue from the album-card context menu on local
  and Plex grids (#636).
- Collapsable multi-artist header on the local album detail.
- New "All" mixed feed with TrackCard, ownership-aware playlist cards, artist
  playlists, a library toggle, and local favorites; genre and sort controls.
- Proper "All" list rows with rich rows and local art/badges; a LabelCard; live
  pins; a live favorite/follow sweep; Go-to-album.
- Fixes: NVIDIA seam, close-to-tray / tray-restore crashes.

## Home & discovery

- Most Played Albums — a local play-count album store fed on every track
  start; a carousel on Home and For You (off by default, self-hides while
  empty) and a View-all page with search and per-card play counts.
- Label pages reach web parity: header actions, a catalog/library toggle and
  label cards.
- Pinned section — a per-user store rendered as a mixed carousel with pin
  affordances.
- ArtistGridCard for mixed carousels, used app-wide, and in the Scene view with
  genres as the subtitle.
- Qobuz Playlists "View all" page with shared header tools; standalone
  release-note links in What's New render as clickable.
- Sort control for the Library Albums rail; auto-refreshing recently-played
  rails with a row refresh button.
- Advanced sub-genre filter sends raw ids to the server (Tauri parity); artist
  follow state corrected in the Home/ForYou Pinned carousel; persisted artist
  follow set no longer wiped on a failed fetch.
- Open an album from a grid-card title click.

## Playlists

- Drag-and-drop reorder for custom-order tracks using the shared row drag
  gesture and drag ghost.
- Optimistic sidebar rename that holds until the list agrees.

## Themes & interface

- Catppuccin Latte, Frappé and Macchiato added, thanks to
  [@TerminalTilt](https://github.com/TerminalTilt) (Latte correctly a light
  theme; registry count fixed).
- Searchable theme dropdown that swaps its icon and narrows the list.
- std-widgets Palette synced to the active QBZ theme.
- Track-row play glyph and CircleAction buttons legible on light themes.
- Track context menu unified into one shell-level instance; album-card "more"
  menu anchored at the cursor.
- Hover tooltip sized to its content so multi-line fits; sidebar dock margins,
  queue-count footer, macOS NPB-small corner.
- macOS: traffic lights centered in the header (reached through the content
  view's window); NPB-Small bottom clearance so the meta line clears the
  window's bottom bezel, Rust-tunable via `QBZ_NPB_SMALL_EXTRA`.

## Scrobbling

- Skip tracks shorter than 30 seconds; do not scrobble when the track duration
  is unknown; the delay gate hosted in `qbz-app` for unit testing.

## Offline & cache

- Reject truncated downloads when Content-Length is known; download size
  validator at module scope.
- Purge only allowlisted cache-layout directories; "Clear cache" clarified to
  keep downloaded albums.

## Settings, security & internals

- Settings bundle engine with importer-side classification; the export modal
  wired to it.
- Flatpak/Snap sandbox settings section with copyable permission commands.
- Credentials: secret files written mode `0600` and re-tightened on load; Last
  .fm auth token prefixes no longer logged; secrets redacted on stderr and
  Last.fm body dumps stopped.
- Headless playback driver with a pure `plan_tick` and a thin IO shell (the
  daemon's engine); duration==0 end-detection edge pinned.
- Qobuz: 403 circuit breaker with prefetch backoff; backoff between
  bundle-extraction retries.
- Dropped Tauri-only boot/graphics/settings modules.

## Packaging & CI

- `test-crates` workspace test workflow (non-UI crates) on pre-release
  integration; qbzd slint-free dep-graph gate.
- Visual build progress in `slint-run` (start/ETA/ticker/final).
- The apt repo update triggers from the aarch64 workflow too (#648); aarch64
  qbzd cross-build script.
- The in-repo Flatpak twin manifest drops the `org.freedesktop.secrets`
  talk-name, matching the published Flathub manifest.
- Dependency bumps (security and maintenance).

**Full changelog:** https://github.com/vicrodh/qbz/compare/v2.0.1...v2.0.2
