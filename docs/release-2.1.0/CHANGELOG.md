# QBZ 2.1.0 — Technical Changelog (v2.0.2 → v2.1.0)

~750 commits on `pre-release` (2026-07 → 2026-08). This release replaces the
Slint frontend shipped in 2.0.x with a native Qt/QML frontend (`qbz-qt`,
cxx-qt bridges) running in the same single Rust process, and ships the first
Windows build. The frontend-agnostic core crates carry over unchanged in
architecture; the protected audio path (ALSA direct / PipeWire / Pulse /
JACK, bit-perfect) was not modified by the frontend swap. Sections group work
by area rather than by commit date.

---

## 1. Architecture

- **Slint frontend retired** (`69ed61207`): the `qbz-ui`, `qbz`,
  `qbz-slint-common`, `qbz-dac-wizard` and `vendor` crates left the live
  workspace. The DAC wizard's headless logic survives as
  `qbz-dac-wizard-core`. With the ~1.6 M-line generated Slint module gone,
  the 20–30 GB compile wall is history — a full release build now takes
  minutes on standard runners.
- **New frontend crate `qbz-qt`**: Qt Quick/QML UI over cxx-qt bridges,
  built up in phases 0–23 (`469f83781` … `9a4fec6b7`) from login shell to
  full parity, then split out of the initial God-object into per-domain
  bridges (`ade156a72`, `5c2577c7c`, `fa433d465`).
- **`qbz-source` — the source-agnostic media seam** (`f1b424134`): one
  registry that resolves every play/queue/artwork request by provenance
  (Qobuz, local, offline cache, Plex, Jellyfin, Subsonic, disc), with
  `PlaybackTicket` as the single audible step (`a55e42633`, `7113ef579`).
  My QBZ and mixtapes were rewired through the seam (`b6cf06ff2`,
  `d9ed19eb6`).
- **New crates this cycle**: `qbz-jellyfin` (`e578d8a54`), `qbz-subsonic`
  (`0f41c44da`), `qbz-media-cache` (`0dfec607c`), `qbz-local-catalog`
  (`a0d8a7fc9`), `qbz-disc` (`db0999bf6`), `qbz-rip` (`5d3d51a88`),
  `qconnect-lan` (LAN discovery/pairing service, merged in `28fe63514`).

## 2. Qt frontend port

- Full parity rebuild against the Slint reference: shell, discover/home,
  library, album/artist pages, settings 1:1 (`83999fc55`), playlists and
  My QBZ (`a16cf56c8`, `acf23abc9`), blacklist manager (`494596616`),
  purchases (`7d9d3b896`-era, `475f261dc`, `7d98ad044`), awards pages
  (`dc2496e5c`), HiFi Wizard (`8ba948346`), immersive mode (B1–B5,
  `b5298b78b` … `c0f4d2f1b`), search cortinilla with local sections
  (`15610bc7e`, `a7e64c0bd`), hotkeys with a cheatsheet and customize editor
  (`0cca9abc5` … `07b0ba818`).
- **Miniplayer and tray**: borderless mini window with queue and lyrics
  surfaces (`cd89b4094`, `684fffd7f`), system tray via ksni (`56d438717`),
  close-to-tray on every exit path (`088387f2c`), window position restored
  after un-hiding (`f64b8480d`).
- **MPRIS**: the desktop sees the player again (`5dd450fb9`); `Position` is
  computed at read time and `Seeked` is emitted, so external widgets stop
  extrapolating a frozen position (`f45b703c6`, `7ba07ec6f`, `8bc32a91a`).
- **Session persistence**: view triple, window geometry, playback state and
  per-page scroll positions restored across restarts (`c2944db97`,
  `89c54cd5b`, `e816436fd`, `221572b36`).
- **Kiosk profile**: a zone-navigated full-screen shell with its own NavRail,
  card primitives and eight views, plus a live toggle (`87c20fce8`,
  `221f8704d`, `63e36c754`).
- **i18n**: live language switching across the 8 bundled locales via the
  `trRev` dependency pattern (`245eaa4d4`); `qbz-i18n` owns its catalogues
  (`1352e499f`).
- QoL: Excel-style Shift multi-select on every surface (`750c59034`),
  an app-wide typeface setting (`3c4c2dae2`), system-font script fallback
  (`82953c72d`), artwork fade-in on arrival (`58fa77a39`), desktop
  notifications only where a toast can arrive (`27022fbf4`) with fresh
  portal ids per track on Plasma (`f6574cdb9`).

## 3. Windows port (PR #725)

- First-class Windows build: WiX MSI packaging with an install/launch/
  uninstall smoke in CI (`8fb02401f`, `783dbb964`, `9a0dbd60c`), custom
  title bar with a native hit test (`415cba57d`), single instance and
  `qbz://` protocol deep links (`afa32d9bb`), session persisted when
  Windows ends it (`3b032e8c4`), miniplayer keep-on-top (`c2272ad9f`), an
  as-is disclaimer shown once per version (`2e0e29277`).
- **WASAPI exclusive-mode** sample-rate probe and hotplug watch
  (`c068b2fd6`).

## 4. Local Library, catalog & media servers

- **Derived local catalog** (`qbz-local-catalog`): Albums, Artists and
  Tracks page from a reconciled per-source catalog (`1bb3c41dc`,
  `299dcde5b`, `6fd83c42d`, `7689fb0df`), with incremental local scans
  (`480b4b46d`), incremental Plex section sync (`6e0e0d316`),
  generation-tagged Subsonic sync (`6005510bf`), background Jellyfin quality
  hydration (`8fff37a8e`), recovery guardrails (`ed8d11e5c`) and album-version
  unification across sources (`d627560f4`).
- **Jellyfin and Subsonic integrations**: protocol crates verified against
  live servers (`e578d8a54`, `0f41c44da` — the Subsonic HTTP-200 error trap
  is enforced in code), `JellyfinSource`/`SubsonicSource` through the seam
  with the full chain walked against both real servers (`b09f03b73`,
  `51aa8ba08`), one shared media cache table (`0dfec607c`), filter chips in
  Local Library (`322b9374f`), merged into both local search paths
  (`13e938727`). Playback from both verified bit-perfect by checksum.
- **Library Explorer** (`9c22a909d`): tree-style browsing across every
  source, with drag-and-drop of rows into sidebar playlists and the queue,
  a collapse-caret tooltip and a collapsed filter strip (`c37768459`).
- **Local tag editor** restored and expanded: split editor workspace,
  canonical write verification, remote metadata lookup (`0e5383758`,
  `75a668ed1`, `d81f69113`), scoped track-row selection (`fa649394d`),
  completed editor sessions (`5a14eef25`).
- **Multi-disc albums**: a divider that names and illustrates each disc
  (`fd25593e3`); SACD image tracks persist in the library (`23a4eb291`).
- **Favorites freshness** (#690): favoriting an album shows up in Library
  without a restart (`bb1b1a564`).
- perf: covers no longer drag their embedded artwork through `lofty` just to
  read tags (`34d19211d`, 9.3 s → 0.9 s on a large scan), high-cardinality
  views windowed and virtualized (`e0e9566f8`, `c6d142ccb`), Tracks tab
  pages append without a model reset and keep the viewport (`fcb985173`,
  `f8092db81`).

## 5. Discs: CD, SACD and ripping

- **CD-DA playback** off a real drive, measured before it was written
  (`db0999bf6`), with a platform-split reader for macOS (`8d4ed8e95`); a CD
  track is a source ticket of its own, not a file (`3ba476b43`).
- **SACD images**: the stereo area reads and plays (`06405969e`,
  `452589dda`); the variable per-sector header and the byte-interleaved
  stereo layout are handled correctly (`70defb2e8`).
- **Ripping** (`qbz-rip`): CD → tagged FLAC, bit-exact and seekable
  (`5d3d51a88`), with a rip wizard, metadata correction pane (`2273db918`),
  remote album metadata search (`b6deec0aa`), a provenance log and cover
  beside the tracks (`b69a058bc`), and disc memory after eject
  (`57ae46566`).

## 6. Playback & audio

- **ALSA hardware volume** (new, guarded — #726 hardening): capability-probed
  enumeration of simple-mixer elements with typed identities
  `(name, index)`, per-device+route persistence keyed by udev identity,
  UCM read-only hints, a closed selector when more than one candidate is
  valid, and fail-closed behavior on ambiguity or disappearance
  (`0e8614fe3`, `96342e23b`, `c3556fe3e`, merged `c059c12bf`). Exposed in
  Qt Settings and the qbzd CLI/TUI. The mixer opens on the card ctl id, not
  the PCM alias (#659, `def93528b`).
- **Playback recovery**: per-attempt total deadlines, transient retries and
  monotonic track-identity signals for source failures; the queue advances
  even if polling missed the stopped pulse — an unreachable track can no
  longer stop playback (`99722aeda`, merged `bb43dea5b`); audible state is
  preserved on source failures (`ecae778d5`).
- **Stereo scope visualizers** (`d11e94622`), with zero-frame packet
  skipping (`6046a959c`). (The seekbar-waveform experiment from the same
  round ended up parked and hidden — `a92dafa73` — and is deliberately not
  listed as a feature.)
- **ALSA Direct visualizer alignment**: ~10 ms pacing aligned to the
  playhead via `snd_pcm_delay` (`eff9c484e`).
- **CMAF**: the superseded track's segment feeder is cancelled
  (`a561e7ae2`), stale gapless pending state cleared on PlayStreaming
  (`740adf55d`), fetch timing split into send vs body (`63b4082e6`).
- **DSD**: conversion cancelled when its track is superseded (`a0ed0d752`);
  `AudioFormat::Dsd` round-trips through the database instead of decaying
  to Unknown (`6328994c0`); the quality badge classifier learned DSD
  (`71f8d0897`).
- **Listen log** (merged `a3161254e`): a per-user listening event store and
  tracker with reading rules, a Listening history toggle and clear in
  Settings, cross-source identity capture (ISRC / MusicBrainz recording ids
  from tags, Plex, Jellyfin and Subsonic), and scrobble start-stamping timed
  by audible position. A gapless hand-off closes the previous row as
  natural, not skip (`7b05ef55a`). Groundwork for offline recommendations.
- **Pulled tracks**: tracks Qobuz removed are marked and kept out of the
  queue (`7780efa5d`), with an available-version finder (`b2c74c414`) and a
  menu that offers nothing leading nowhere (`e779b6ec1`).
- Offline and logged-out scrobbling restored (`aadad1da5`); player volume
  persists across launches (`b8b493bd5`).

## 7. Qobuz Connect

- **LAN parity** (merged `28fe63514`): the official-client LAN service —
  HTTP + mDNS announce, delegated credentials kept isolated, a transactional
  session coordinator — so official Qobuz apps discover and pair with QBZ on
  the local network, always-on with the global QConnect switch, exactly like
  the official clients. The wire contract was normalized from real captures
  (APK/Electron/web bundle).
- **Hardening**: real buffering states (`PLAYING+BUFFERING` intent
  preserved), duration/offset corrections, authority fences and bounded
  teardown for Qt and qbzd, last-wins takeover, and exact Cast↔QConnect
  transfer with synchronous epochs. Shuffle materializes the WS seed exactly
  (xoshiro128** + official Fisher-Yates), remote volume/mute project onto
  the now-playing state, and ceding to another renderer waits for
  `ACTIVE_RENDERER_CHANGED` before stopping. Race-sensitive logic lives once
  in `qconnect-app`, not per-frontend.
- Verified in smokes against the official iOS client and Web Player:
  identical queue order on all three, volume both ways, handoff without
  pausing the session.

## 8. Casting

- **Round two** (merged `44c108d4d`): progressive serving, clicked-track
  routing, a source proxy so Plex/Jellyfin/Subsonic tracks cast too, and a
  clean shutdown path.
- **Single remote-renderer seam**: `route_play_remote` (cast → peer → local)
  behind every play entry point, a typed `NoMediaSession` idle poll and a
  volume coalescer (merged `95f2d6519`).
- **Visualizer while casting**: a shadow decoder feeds the scopes/spectrum
  for proxied and direct tracks while audio renders on the cast device
  (merged `3fc23aea4`, `41b8e7e6f` — the latter also fixes DLNA
  stop-before-load).
- **X.509 v1 device certificates** accepted in the Cast TLS handshake
  (#730, merged `e9ba7a7bf`): `rust_cast` vendored with a 38-line delta,
  verified byte-for-byte against crates.io — older Chromecast built-in
  devices work again.

## 9. Playlists & queue

- **Add-to-playlist redesign** (merged `4c4647bdf`): a membership index with
  a background hydrator, a picker showing "Already in" and the last-used
  playlist, local drag-and-drop, Jellyfin/Subsonic tracks in playlists, and
  quality badges with tooltips.
- **Album Quick View** and queue focus controls (`d555b8fde`).
- **Play later, everywhere** (merged `ea02cc238`): album-level enqueue takes
  the block-tail arm (#442) so "later" means after the current block;
  "Play all later" on the Popular Tracks group.
- **Extended queue view** (`c7038a685`), drag-and-drop insert at position
  (`c198afb33`), reorderable history occurrences (`4630c4022`), playback
  history persisted across restarts (`a2d0ada42`).
- **Importer expansion**: playlist files, JSON, ListenBrainz and Last.fm as
  import sources (`7e233104f`), reachable from the Playlist Manager toolbar
  (`9899ad6b0`).
- Mixed playlists open offline (`b4744b6e7`); playlist cover editing
  restored (`aa66c58ea`); custom covers propagate to every surface
  (`d112c726b`).

## 10. Search & cortinilla

- Local sections in the search cortinilla with click routing into the
  LocalLibrary tabs (`15610bc7e`, `4806584ee`), instant cached first paint
  (`b38b0b3d5`), module-OFF results fallback
  (`10f249be2`), debounce moved from QML into Rust (`5a194d249`).
- Focus-driven lifetime instead of idle timers, a Cut/Copy/Paste/Select-all
  menu on the search input, and "Open containing album" on track rows
  (merged `ea02cc238`).
- Tracks results use the app's full track row (`7fc760588`); section rows
  can open their first tab on click, opt-in (`e9f09a59a`).

## 11. Immersive & visualizers

- Immersive port and expansion: root overlay and header band (`8bc255269`),
  atmosphere and focus panels (`46c1c4240`), split view with lyrics, track
  info, suggestions and queue tabs (`0b6e8ed7c`), player bar and in-immersive
  search cortinilla (`c0f4d2f1b`), shader scenes with RHI items and GL link
  pairing (`e6c058c2f`), artist scene with the shared scene-discovery engine
  (`016eb3cd2`), remembered shader scene (`47cab502f`).
- Visual cleanup round (merged `0b5ce47f2`): polarity-aware ambient veil for
  light themes (`2e3c76561`), overAmbient legibility for the quality badge
  and track info (`949399312`), accents derived from the album palette
  (`1a4797085`), reworked cinematic split-card metadata (`460c0cca4`),
  karaoke dim of the unsung line part (`68eeb1017`), minimize/maximize in
  the window capsule (`aeeb55198`).

## 12. Performance — the GPU campaign (95% → 25%)

Full investigation:
`qbz-nix-docs/qt-frontend/2026-08-11-scenegraph-batches/GPU-COST-INVESTIGATION.md`.

- The shell (dynamic background + Large-bar visualizer) went from **93–97%
  GPU / ~34 W to 25–26% / ~13 W** measured on the reference hybrid laptop —
  better than the Slint 2.0 UI (35–56%) on the same machine (`f46c5c95d`,
  `a2a99a72c`). Root cause: full-window present rate, not render cost — Qt
  Quick has no partial repaints, and with KWin compositing on the dGPU every
  present costs ~1.2% GPU flat, area-independent.
- **One repaint pulse for the whole shell** (`QbzShell.pulseMs`, ~30 Hz,
  `QBZ_PULSE_MS` knob): background atmosphere, visualizer and lyrics engine
  tick on the same edge. Standing rule: no continuous animation owns a
  `Timer`/`NumberAnimation`/`Behavior`, and a frozen or invisible component
  writes nothing.
- Leaks closed: the ambient FBO double-presented; immersive panels (mounted
  while the overlay is closed) wrote on every FFT publish; 100 ms Behaviors
  animated at display rate (immersive 83% → ~25%).
- Blurred background mode composes its four layers + dim + scrim in **one
  opaque ShaderEffect** with verified numerical parity; the image stack
  remains as the software-renderer fallback.
- The animated now-playing row indicator returns (eq bars behind the
  `play-indicator-animation` pref), mounted on the pulse at zero extra
  presents (`eb80e2f36`).
- Elsewhere: tabs build only what is showing, with a build-gate that fails
  on hidden-tab construction (`a4c29c796`, `a6b373cb5`); navigation cost is
  measurable headless (`801543930`); route changes acknowledged on the next
  presented frame (`5b274cab5`); two-phase route commit cut dead-click time
  590 → 73–90 ms; the local albums grid actually recycles (`bd9aec144`);
  album page track list rides a uniform-cell ListView (`047db8ea9`); a long
  restored queue no longer freezes launch (`66ba12bd5`).

## 13. qbzd

- **Event hooks** (PR #700, Filippo Vicentini, merged `e109e88ec`):
  `hooks.script` (or `QBZD_HOOK`) names an executable the daemon forks once
  per daemon event, the event described in `QBZ_*` environment variables —
  the pleezer/shairport-sync push-integration contract, aimed at moOde,
  Volumio and DIY boxes (`ee1bbf9b7`), with hardened hook execution
  (`251f153a6`).
- **Gapless robustness (#699, Pi 1 GB)**: the successor warms on every track
  start and cold gapless hand-offs stream instead of buffering
  (`e90ea8cad`), gapless fetches stay off the driver loop (`eb643b9d3`),
  plus inhibit backoff (merged `51b7e012f`) — smoke-tested on a real Pi.
- **ALSA hardware volume** controls in the CLI/TUI, same probe and closed
  selection as Qt (`c3556fe3e`).
- **QConnect**: qbzd shares the hardened `qconnect-app` core (shared state,
  volume mode, task registry adapters), with a hardened ownership lifecycle
  (`c927eb9c9`), the quality cap enforced for QConnect (`5bae44286`), and a
  cold load of the current track on a state-only resume (`7f9397f91`).
- **State reliability**: bridge state changes deduplicated on the mapped
  three-way state (`e336d2d1d`); `TrackStarted` re-emitted when the same
  track replays after a stop (`8ec1f3c27`); per-OS instance lock gate
  (`f06ffc714`).
- **Packaging & deployment**: standalone qbzd packages with IP masking in
  logs (`efa486ba4`); appliance and read-only deployment documented
  (`6bfeb88c3`); glibc floor 2.35 on both arches so the Pi streamer keeps
  working (`51daf760b`).
- **Misc**: low-memory profile re-wired (#660, `a69491a49`); process-level
  rustls `CryptoProvider` installed (#663, `a01097708`); a listen-log
  subscriber beside the scrobbler (`3e060d653`); the artist-vector store
  logs loudly when it fails to open (`6587201a1`).

## 14. Community-contributed fixes (2.0.x issues)

- Real desktop keyring backends for secrets (#697, `b60ed2c26`) and a
  mobile KDF fallback (#695, `4d8779e17`).
- Relocatable CMAF offline bundles (#696, `0a934ae4a`) and a configurable
  bundle cache directory (#708, `9aeee886d`); atomic bundle cache
  replacement (#707, `388a3591e`).
- Optional mixtape source resolution (#694, `910bee49a`); lofty unified at
  0.25.1 (#679, `c039f0d0e`, `b6c69509d`); qbzd rustls TLS provider
  (#663, `fe2be63f0`); ALSA buffer/period sizes as `alsa::pcm::Frames`
  (`82fea6257`); renderer preference honored with an empty backend env
  (#720, `f9b139b44`); preferred-GPU startup hardened (`79f91492b`).
- Hotkeys + Vim keymap (PR #724, Niklas Herder, merged `e993bc1e4`).
- Contributor credits swept into README and About (`b2d3656eb`,
  `fbc8e9fa3`, `acc0e3302`); About gained a Sponsors section
  (merged `ea02cc238`).

## 15. Build, CI & packaging

- **Release workflows rewritten for Qt** on standard GitHub runners:
  reusable Qt Linux build (`929ce39f0`), Linux + aarch64 (`149c021e4`),
  macOS with aqt Qt + macdeployqt (`7f98e7b9a`), Snap packs the AppDir and
  the Launchpad source recipe is removed (`89030b5b2`), Flatpak repacks the
  bare binary on the KDE runtime (`c6bfb90ed`), Windows release workflow
  with WiX authoring and LGPL notices (`8fb02401f`).
- **test-crates as a living regression gate** with `scripts/cargo-test.sh`
  as its exact local mirror (`ce57551a7`), a Qt gate with the QML audits
  shipped in-repo (`077e5fd97`) — six audits including shader bake
  (`af8d373d2`) and baked-icon coverage (`ffc2d7d66`) — and a shared
  qmllint gate across Linux and Windows (`bf710fb7e`).
- **glibc floors**: x86_64 2.35, arm64 desktop 2.39 (aqt's arm64 qmake
  segfaults on 22.04-arm), qbzd 2.35 everywhere, enforced arch-aware in CI
  (`51daf760b`).
- Desktop-entry ↔ MPRIS ↔ icon invariant checked against recipes and built
  packages (`a1763df25`); macOS Liquid Glass icon via Icon Composer
  (`8b77f1ae0`); hardened AUR/Gentoo/updater-manifest publishing
  (`ac506e4d6`, `aaf9573c5`, `ecd4fd05c`); MSI smoke verifies both URL
  protocols (`353ed7fff`).
- README rewritten for the Qt frontend (`08d924e1b`).

## 16. Dependency bumps

- `cargo update`: 194 semver-compatible bumps, plus rand 0.10,
  roxmltree 0.21, base64 0.23, prost 0.14, tokio-tungstenite 0.30,
  mpris-server 0.10, mdns-sd 0.21, rfd 0.17 (merged `7d9d3b896`).
- **rusqlite 0.40**: explicit i64 casts (~40 sites); its multi-statement
  `execute()` rejection exposed a latent 0.31-era bug — the second statement
  of the editions prune never ran; now `execute_batch`.
- **reqwest 0.13**: feature renames, `query`/`form` opt-in; live HTTP paths
  re-smoked (login, streaming, scrobbling).
- Deferred to 2.1.1 (coupled to the protected audio stack or multi-day):
  symphonia 0.6 + rodio/cpal/alsa wave, cxx-qt 0.10, keyring 4, the
  RustCrypto wave, ringbuf 0.5.

---

Full changelog: https://github.com/vicrodh/qbz/compare/v2.0.2...v2.1.0
