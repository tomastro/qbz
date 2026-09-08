# QBZ 2.0.0 — Technical Changelog (v1.2.15 → v2.0.0)

824 commits on `slint-mvp` (2026-05-18 → 2026-07-03). This release replaces the
Tauri 2.0/WebView (SvelteKit) frontend with a native Slint UI running in the
same process as the Rust core — no web engine, no IPC bridge. Nearly every
feature below was rebuilt from scratch against the new shell; the sections
group work by area rather than by commit date.

---

## 1. Architecture

- **Frontend rewrite**: the SvelteKit/WebView frontend is replaced by a native
  Slint UI (`qbz-ui` library crate + `qbz` binary crate). Single Rust process,
  no embedded browser, no Tauri IPC — the UI calls into the core crates
  directly (`f1748383`, `c8ef2a1b`).
- **`qbzd` removed**: the dormant standalone TUI/daemon crate was dropped from
  the workspace; it pinned an old `unicode-width` that blocked the Slint
  upgrade past 1.14 (`eddfaed4`).
- **Frontend-agnostic core crates extracted from `src-tauri`** (ADR-005/006 —
  no legacy dependency, no `tauri::State` in core logic), so the same code
  runs headless (Slint, future TUI/CLI):
  - `qbz-credentials` — keyring + AES-256-GCM encrypted token store (`aa64f3fa`).
  - `qbz-radio` — smart-radio builder/db/engine (`4a9cbdeb`).
  - `qbz-offline-cache` — offline download pipeline, metadata/migration,
    `CacheEvent` sink, purge logic (`d6e9345f`…`c8158bf4`).
  - `qbz-plex` — Plex integration, ~1700 lines moved out of Tauri (`2fe7008e`).
  - `qbz-mixtape` — Mixtapes/Collections backend (`5458f3aa`).
  - `qbz-lyrics` — synced-lyrics provider chain and cache (`46c15f50`).
  - `qbz-theme` — theme/token registry with WCAG/APCA contrast math (`ff5536a0`).
  - `qbz-i18n` — pure-Rust gettext bundling (`f92c95ee`).
  - `qbz-reco` — recommendation/taste-vector engine (`65de9849`).
  - `qbz-dsd` — DSD demux/decode/output engine (landing in the 2.0.0 tag).
  - `qbz-dac-wizard`, `qbz-text-utils`, `qbz-slint-common` — further splits off
    `qbz-ui` for build parallelism (`7678bceb`, `6f9ad466`, `d1437c70`).
- **In-process communication model**: state that used to cross the Tauri IPC
  boundary (`tauri::State`, `invoke`/event channels) is now shared directly
  between the Slint controller layer and `QbzCore`/`qbz-app` — no serialization
  boundary between UI and playback/session state.
- **Qobuz Connect cross-frontend seam**: a 16-method `QconnectRendererEngine`
  trait is implemented once by each frontend (`CoreBridge` for Tauri,
  `SlintRendererEngine` for Slint) so renderer orchestration (echo-seek
  rejection, cursor align, queue materialize) lives in `qconnect-app` a single
  time instead of being re-derived per shell (`e5e8d4a8`, `e8ac6185`, `88e72c3e`).
- **CMAF streaming, playback caches, and the offline-tier resolver** moved into
  the frontend-agnostic `qbz-player`/`qbz-core`, so both frontends share one
  playback path (`29b6c205`, `d0a4a329`, `506ba5ef`).

## 2. UI shell & views

- Three-state sidebar (open/mini/closed), right-flyout section nav, live theme
  switching, Discovery-style segmented tab bar, browser-like back/forward
  navigation history (`682719ca`, `12ed5a7c`).
- **Custom title bar**: frameless window with drawn controls, drag-to-move,
  double-click-to-maximize, edge resize (Linux opt-in, macOS overlay default
  on), plus an opt-in compact icon-only header nav (`a7f538b3`).
- **Theming**: 24 standard themes transcribed 1:1 from the old `app.css` into
  a Rust registry, alpha polarity resolved per-theme rather than by a flat
  light/dark flag, plus 6 redesigned high-contrast accessibility themes with
  WCAG/APCA contrast tests and a broad `Theme.is-high-contrast` tokenization
  pass across sliders, tabs, toasts, badges and focus rings (`40bd77f7`,
  `48eefa3c`, `ff5536a0`).
- **NowPlayingBar**: rebuilt with four selectable layouts — New, Classic,
  Small ("Mode C", 42px compact row with floating art-hover preview), and
  Large ("Mode B") — plus a borderless **miniplayer** window with 5 surfaces
  (micro/compact/artwork/queue/lyrics), mirror-driven state, no extra poll
  loop (`da7758b9`, `1ad0d7c1`, `8fa03a8c`, `9d6b3533`, `c974b8a9`).
- **System tray, MPRIS/SMTC media controls, and notifications**: Linux tray via
  `ksni` (Wayland-safe hide/show, since winit's `set_visible` is a no-op on
  Wayland), macOS/Windows tray icon (later hand-rolled objc2 menu for correct
  icon sizing), MPRIS on Linux and SMTC/MediaRemote on Windows/macOS with
  content-sniffed album art, now-playing system notifications (`8a78c225`,
  `a91d629c`, `7a7e5f82`, `d08c5f65`).
- **UI scale presets** (XS/S/Default/L/XL via a settings-driven scale factor)
  shipped for interface density (`b7549790`).

## 3. Playback & audio engine

- **CMAF streaming** ported into `qbz-player`: `play_track` fetches the init
  segment first, derives sample rate/bit depth, then background-fetches and
  decrypts subsequent segments, falling back to the legacy path on setup
  failure (`29b6c205`, `74ab627c`, `655a90f4`).
- **Gapless + prefetch**: L1+L2 instant-replay cache and gapless-prefetch
  helpers wired into the Slint playback controller (`7b84fa9a`, `3a6aae19`).
- **Device routing (issue #263)**: a three-tier fix stops the PipeWire global
  clock `force-rate` write from firing while a device lock is held, fixes a
  `clock.force-rate` leak on stop, resumes a suspended sink after exclusive
  access ends, and finally routes locked output to the actually-selected sink
  via `PIPEWIRE_NODE` instead of silently following the system default
  (`d19312fe`, `b139392b`, `64fb77ad`, `d2c4aaf9`).
- **New JACK backend**: lock-free SPSC ring buffer, allocation-free RT process
  callback, feeder thread resampling to the JACK graph rate, auto-connect to
  physical ports — offered as an opt-in backend, explicitly not bit-perfect
  (single fixed graph rate) (`fbc03f41`, `68bc9bd6`).
- **PulseAudio works out of the box** alongside the existing ALSA/PipeWire
  backends, with output-device list dedup, a searchable device selector, and
  Flatpak `--device=all` for direct `/dev/snd` access (`c75a0af5`, `bdb9cada`,
  `f98d92d8`).
- **Bit-perfect** playback preserved throughout the rewrite: format-first
  quality badges, per-device bit-perfect-capable marking, and a HiFi Wizard
  self-test with bit-perfect read-back (see section 12).

## 4. DSD support (landing in the 2.0.0 tag)

New `qbz-dsd` crate with in-house DSF/DFF demuxers. Three playback modes are
selectable in Settings (ALSA backend):

- **Convert** — dsd2pcm (Gesemann-derived 6x256 LUT engine) with halfband
  decimation down to 88.2 kHz/24-bit PCM. Works on any output device.
- **DoP** — DSD-over-PCM packing (0x05/0xFA markers over an S32 carrier) via
  direct ALSA hardware access, with gapless DSD playback and a pause mode that
  keeps the DAC locked using DSD silence bytes.
- **Native** — raw DSD_U32_BE/LE output for DACs and kernels that support it,
  covering DSD64/128/256.

Also included: ID3 tag and artwork extraction for DSF/DFF, library-scanner
recognition of `.dsf`/`.dff` with DSD64/128/256 quality badges, 5.1 SACD rips
played back via ITU BS.775 stereo downmix in Convert mode, and a local-FLAC
gapless bug fixed along the way. This work lands just ahead of the 2.0.0 tag
and is uncommitted at the time of writing.

## 5. Search

- **"Intelligent Search"**: a frontend-agnostic `SearchService` in `qbz-app`
  with a result cache (Capa A) and interaction-based ranking (Capa B), a live
  header dropdown with skeleton loading, stale-while-revalidate, 220ms
  debounce, keyboard nav, per-section caps, and an opt-out setting (`22fb4162`).
- **Search "cortinilla"** — a glass search overlay for Albums/Artists/
  Playlists that swaps playback without leaving Immersive mode, with a
  configurable action (replace queue / play next / add to queue), later
  extended to Local Library, Plex, and per-source artwork (`22fb4162`,
  `b1ac402c`).
- Hardening: the dropdown no longer resurrects on navigation, auto-closes
  after 4.5s idle, and a typing guard was extended across remaining text
  inputs so hotkeys don't fire while typing (`12b1b312`, `31436325`,
  `c65bd9c2`).

## 6. Offline mode & Local Library

- **Offline mode**: a new frontend-agnostic `qbz_app::offline_mode` state
  machine (Online / RealOffline session-scoped / InducedOffline persisted)
  backed by a layered connectivity actor (default-route signal, audio-liveness
  window, vendor-diverse probes, hysteresis, captive-portal/suspend-resume
  handling), with all Qobuz HTTP traffic routed through a single offline gate
  (`edae9d24`, `ba702bc3`). Slint wiring covers offline session entry, a
  recovery banner, tri-state Settings, offline artwork policy, disk-first
  cached favorites, first-class local/offline-only playlists, and QConnect
  offline gating.
- **Local Library** (new feature area, folder-based/self-managed music):
  server-paginated Albums/Tracks tabs with local+offline playback and
  source-aware auto-advance, Folders tab (flat then flat+tree), Artists tab as
  a two-column master/detail browser, folder management in Settings, and a
  library scan engine with progress (`835a6cad`, `56b3ca79`, `df351ec6`,
  `aa7c320b`).
- **Tag editing**: a full tag editor built on `lofty` (sidecar-default,
  opt-in write-to-file), a manual-edit modal, and remote metadata lookup
  against MusicBrainz + Discogs (`7e0d970f`, `4e79386d`, `3c2f46b7`).
- **Plex integration** ported end to end: credential store, Settings auth/PIN,
  albums/artists/tracks in Local Library, progressive/fast-play streaming,
  source-aware artwork everywhere (now-playing bar, queue, MPRIS),
  server-transcoded thumbnails, and same-title album disambiguation into
  selectable versions (`f7619a92`, `a45840d1`, `06b2bea9`, `334ddf31`).
- **Offline cache manager**: a Settings view for remove/redownload, covers,
  sort, and a failed-only filter, plus per-row offline buttons on every track
  row and whole-playlist caching (`3e0a718e`, `a7f2283c`).
- **Scrobbling**: Last.fm and ListenBrainz scrobblers wired to the player and
  Settings (`952846b6`).

## 7. Qobuz Connect

- **Hardening** (merged `f3d37dac`): unified cast-eligibility checks, strict
  source parsing (unknown sources blocked, not allowed through), mixed-queue
  guards with a Rust-side per-track origin backstop, a 12s renderer-liveness
  watchdog, half-open WebSocket detection via keepalive pong-timeout,
  idle-retry/startup-backoff recovery, takeover arbitration on `SESSION_STATE`,
  and full resync (state + queue) on reconnect (`f3686c19`, `434c2218`,
  `a05e8abc`, `7628c284`, `ed1d7aee`, `6c48266a`).
- **Cross-frontend extraction**: session/liveness logic, `device_uuid`
  persistence, and diagnostics relocated to shared crates; the
  `QconnectRendererEngine` trait now carries renderer orchestration once for
  both frontends (see Architecture) (`f100cf4d`, `e5e8d4a8`).
- **Slint port**: renderer + controller integration, device picker with
  cast-aware now-playing state, peer-renderer playback reflected on the
  now-playing bar, peer volume with a 50% safety clamp, bit-perfect volume
  lock/force-100 parity, per-device-type icons, and takeback resume at the
  handed-off position (`44876d01`, `0d7ac793`, `9bb5e6db`, `8e1bf18f`).

## 8. Discovery & recommendations

- **`qbz-reco`** (new crate, ADR-006): `SparseVector` math, relationship
  weights, a 3-table SQLite `ArtistVectorStore`, and a direct-weight-ranking
  suggestions engine, trained from per-user play events (favorite-add,
  playlist-add) to back DailyQ/WeeklyQ mixes, a Rediscover row, and reordered
  "For You" favorites (`65de9849`, `4828e54c`).
- **External recommendations**: a Last.fm + ListenBrainz → Qobuz resolution
  engine wired as a 4th Discover tab, redesigned artist/album-centric with
  common/recent split and a 48h results cache, plus a Last.fm similar-albums
  carousel on AlbumView (`408b3c83`, `124d97e7`).
- **Playlist "Suggested Songs"** shipped, and a Discover section configurator
  lets users reorder/toggle Home/For You sections (`2e1d8e43`, `2a37f2ad`).

## 9. Lyrics

- New `qbz-track-lyrics` Qobuz endpoint with verified wire DTOs, feeding a
  headless `qbz-lyrics` engine: Qobuz-first provider chain, LRCLIB (search +
  scorer) fallback, then lyrics.ovh; LRC parser/emitter preserving word-level
  wsync, a per-user SQLite cache (WAL, additive migration), offline
  cache-only mode, in-flight dedupe, and 55 unit tests (`46c15f50`, `d1203937`).
- Slint UI: static rendering evolved into a full sync engine with karaoke
  fill and auto-center scroll, plus a controls flyout for prefs and cache
  settings (`5a4dbd87`, `5f9fd5bd`, `eea336fd`).

## 10. Content blocking

- **Artist blacklist**: headless service extracted from `src-tauri`, applied
  across search, track rows (greyout/inert), albums/playlists/favorites/
  suggest rows, all play/queue builders, and a dedicated Blacklist Manager
  view (`ef93c864`, `836dd249`).
- **Album blacklist**: a parallel String-keyed `album_blacklist` table
  alongside the artist table, sharing the same enabled flag, enforced across
  the same consumers with a Manager "Albums" tab (`3fbb166b`, `c85ad5f6`,
  `66889af4`).

## 11. Immersive mode & visualizers

- Full-screen "Immersive" now-playing experience (new epic, not present in
  Tauri): reusable atmosphere generation (art-derived blur/vignette), a
  zero-IPC 30fps visualizer feed (`VizFrame`/`VizSink`) lifted off
  `tauri::AppHandle` into `qbz-audio` (`3adb5172`).
- Visual modes: Album Reactive, Static, Coverflow (2D fan), Spectrum
  Visualizer (~2240 to ~28 gradient bars for performance) (`92052a3c`,
  `c067d237`, `03dfd661`).
- **wgpu fragment-shader underlay**: renderer swapped femtovg(GL) to
  femtovg-wgpu, with a WGSL plasma shader driven by the visualizer feed, then
  extended to Tunnel and Aurora "milkdrop" scenes plus a scene picker; macOS
  stays on the Skia renderer since femtovg-wgpu pegs CPU on Metal
  (`e46e8554`, `653a449d`, `fb0aa619`). Later scenes added: Wave Bed, Spectral
  Ribbon, Line Bed (`226c2118`); shader scenes hide automatically on
  non-wgpu renderer tiers (`a050e639`).
- Focus/split layouts: lyrics panel, queue/history panel, track-info panel,
  and a live-query suggestions panel (`77068e3b`…`e6f9c3b4`).

## 12. HiFi Wizard revamp

Built headless-first: native `pw-dump` device enumeration with `pactl`
fallback, DAC negotiated-rate probing via `/proc/asound`, audio-stack health
and distro/init-system detection (systemd/antiX/Artix/NixOS), and
Flatpak/Snap sandbox-aware host detection (`381eafdf`, `f08943ad`, `93da676f`).
Slint UI: a 6-step guided modal (check → select DACs → test → review/apply →
done) with a self-service playback test and bit-perfect read-back (`9bb392c2`,
`9c5febda`, `f87f2986`).

## 13. Internationalization

- New pure-Rust `qbz-i18n` crate: gettext `.po` catalogs bundled at compile
  time (no gettext C dependency), plural rules, and a `t`/`tn`/`t_args`/`tf`
  API (`f92c95ee`).
- Full extraction and seeding pipeline from the old Tauri JSON strings across
  every module (local library, MyQBZ, discover/home/search, artist/album/
  label, playback/offline, integrations/DAC wizard) (`f14c5130`…`e167936c`).
- Live language switching without restart, locale-aware dates, and dynamic
  msgid registration via `mark()` (`69d85f19`, `8243f2a8`).
- Seven bundled locales at full coverage: English, Spanish, German, French,
  Portuguese, Russian, Japanese — the last two (`be487dca`) plus a macOS CJK
  font-fallback fix (vendored `i-slint-common`, `fa3ee1e4`) so Japanese kanji
  render correctly on Mac builds.

## 14. Performance

- **Renderer tier auto-detect** (wgpu → GL → software) with a Settings
  override and auto-revert if a tier misbehaves (`fe111c3f`).
- **Artwork memory bounding**: decoded-artwork memory capped and covers
  decoded at display size instead of full resolution (`f37b6b6a`).
- **Leak/idle fixes**: shader-underlay GPU leak and idle 30Hz wakeups fixed,
  palette computation moved off the UI thread, idle now-playing pushes
  skipped, immersive atmosphere animation stopped while paused (`dbe818c7`,
  `d7716d7c`, `e71f0ded`).
- **Viewport windowing** (phase 1) applied to the Local Library albums grid;
  virtualized `ListView`s (instead of `Flickable` + `for`) applied to
  Favorites Tracks and Playlist track lists for 2000+ item lists
  (`23b572e5`, `85223b03`, `4465a367`).
- **Reduced motion** on GL/software renderer tiers (`d9b82e7b`).

## 15. macOS

- Stable native build with a dedicated dev build/run script (`85518699`).
- CJK font fallback fix so Japanese/Chinese/Korean text renders correctly
  (vendored `i-slint-common`, `fa3ee1e4`).
- macOS Dock policy handling for the tray icon, then a hand-rolled objc2
  tray menu for correct icon sizing/click routing (`a91d629c`, `b9434a50`).
- Overlay title bar as the macOS default, distinct from the Linux
  frameless mode (`a7f538b3`).
- Renderer forced to Skia on macOS since femtovg-wgpu pegs CPU under Metal
  (`fb0aa619`).
- Contributor work from Afonso on macOS-specific fixes throughout the range.

## 16. Build, CI & packaging

- **Crate split for build memory**: `qbz-slint` split into a `qbz-ui` library
  crate (owns the full `.slint` tree + bundled translations) and a `qbz`
  binary crate reclaiming the `qbz` binary name from the retired Tauri app —
  halves the ~20GB single-crate `rustc` peak into two independently cacheable
  units, with further extractions (`qbz-dac-wizard`, `qbz-text-utils`,
  `qbz-slint-common`) (`c8ef2a1b`, `7678bceb`).
- **aarch64 Linux** build script added (native ARM runner + x86-64 cross
  path); documents the ~30GB `qbz_ui` RAM wall that rules out 8GB Macs and
  4GB Raspberry Pis for native builds (`b89ee385`, `5fdbdb3a`).
- **CI rewrite**: GitHub Actions build for `qbz-slint` on Linux stood up and
  hardened (runner disk exhaustion fixed, swap sized for the ~30GB peak,
  mupdf-sys system libs added), then moved to a self-hosted runner for real
  artifact builds (`5806b0c0`…`b9eb3005`).
- All-hosted Snap pipeline added; deb/rpm packaging metadata added for the
  Slint `qbz` binary; release workflows rewritten and channel packagers
  repointed at the Slint binary; release profile now strips symbols
  (`77a476c8`, `9c37e456`, `1b933299`, `6e6153b2`).
- Linux binary kept compatible with glibc older than 2.43 (`cafd521b`).
- Faster local dev builds via the mold linker and a memory-aware
  `slint-run.sh` to dodge the OOM wall (`ec4b10fc`, `14e800ea`).

## 17. Other new features

- **Purchases** ("My Purchases", 13-slice epic): wire models, HTTP + CDN
  fetch, pagination, download state machine, list/detail views with format
  and quality pickers, Settings toggle (default off) (`aceaf9b0`…`9fbfcb55`).
- **Awards pages** ported (landing, album listing, follow) (`9ccfbd38`).
- **Playlist importer** ported (Open Playlist Importer) (`3689a5fd`).
- **Keyboard multi-select** ported and extended to Mix/Label/Local Library
  Albums, with a three-column shortcuts cheatsheet and matching customize
  editor (`53e1e8cd`, `1a9c1327`).
- **Chromecast/DLNA casting**: a shared external-stream resolver
  (CMAF-first, UltraHiRes-first), `qbz-cast` crate, picker modal, and
  offline-gate routing (`fdf2fec3`, `3e1101ff`).
- **Discord Rich Presence** (opt-in) ported from Tauri (`ef38e7a0`).
- **Session persistence** across restarts: startup page, window geometry,
  and playback state (`dd32f573`).

## Fixes

- Output-device routing/locking bugs from issue #263 (see Playback section).
- Local-FLAC gapless playback fixed (landed alongside the DSD work).
- Half-open QConnect WebSocket sessions now detected via keepalive
  pong-timeout instead of hanging indefinitely (`7628c284`).
- Startup now restores the exact last view, not just the top-level tab
  (`e44aa07a`).
- Album credit fixed to read the album-level composer field instead of
  per-track (`0ddcad40`); missing pagination fields in album responses are
  now tolerated instead of erroring (`9e2a1d12`).
- Single-click bug on button primitives (a `FocusScope` ate the first press)
  fixed app-wide (`88f1967d`); single-click album context menus (`8c182875`).
- Secret-vault open moved off the async pool to avoid a zbus panic
  (`40414991`).
- Artist follow heart made a real working toggle, reflected across every
  card surface (`d906e556`).
- Qobuz playlist Follow/Copy state fixed across surfaces, with owner-aware
  remove (delete vs. unfollow) (`bf4ffbd6`, `de32c168`).
- `/radio/*` deserialization repaired and every radio entry point rewired
  (`6520c007`).
- rustls `CryptoProvider` installed at startup for Qobuz Connect (`90705471`).
- Don't cache empty ListenBrainz results, which previously made Weekly
  mixes vanish (`1e78884e`).

---

Full changelog: https://github.com/vicrodh/qbz/compare/v1.2.15...v2.0.0
