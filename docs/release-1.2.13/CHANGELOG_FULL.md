c747c72e chore(release): bump version to 1.2.13 "400+ stars"
bfcd7690 fix(cast): heartbeat the Chromecast every 5s to keep the connection alive
bf734546 Merge bugfix/pre-1.2.13: graphics recommendation cleanup, faster track resume
3abb7f66 fix(playback): gate within-track resume on cache, drop slow seek path
3d29cf13 fix(graphics): make recommendation advisory-only, GPU-aware DMA-BUF help
29b766ae fix(graphics): recommend CPU for NVIDIA-only, allow DMA-BUF on hybrid
f02e1925 Merge branch 'pre-release' into bugfix/pre-1.2.13
420fd4d2 Merge refactor/qbz-app-boot-watchdog: boot watchdog
1a5dbe07 refactor(qbz-app): extract boot watchdog decision
fbf3a5a1 Merge refactor/qbz-app-graphics-autoconfig: graphics autoconfig
3c963cda refactor(qbz-app): extract graphics autoconfig
f457401d Merge refactor/qbz-app-graphics-detection: graphics detection
dceeacb5 refactor(tauri): use qbz-app graphics detection
2fc9270c refactor(qbz-app): add graphics detection
2beb83cb Merge refactor/qbz-app-user-data-paths: user data paths
b65feee5 refactor(tauri): use qbz-app user data paths
a497d61b refactor(qbz-app): add user data paths
e4ec5cd3 Merge refactor/qbz-app-developer-settings: developer settings store
be500956 refactor(qbz-app): add developer settings store
1fe0ac35 Merge refactor/qbz-audio-settings-foundation: audio settings store
87e426ab refactor(tauri): use qbz-audio settings
f85df353 refactor(qbz-audio): align audio settings store
c4a0a52a Merge refactor/qbz-app-graphics-settings: graphics settings store
441bcc5f refactor(tauri): use qbz-app graphics settings
bcdbc699 refactor(qbz-app): add graphics settings store
88432500 Merge refactor/qbz-app-image-cache-settings: image cache settings store
4d1feec7 refactor(tauri): use qbz-app image cache settings
f8f8bb88 refactor(qbz-app): add image cache settings store
62cd6ae1 Merge refactor/qbz-app-download-settings: download settings store
10b0e259 refactor(tauri): use qbz-app download settings
26012e98 refactor(qbz-app): add download settings store
74ce1c32 Merge refactor/qbz-app-remote-control-settings: remote control settings store
faca8376 chore(tauri): update qbz-app lock dependencies
f527186c refactor(tauri): use qbz-app remote control settings
2e438318 refactor(qbz-app): add remote control settings store
ebe03193 Merge refactor/qbz-app-ui-preferences-foundation: qbz-app UI preference stores
7f8e3533 Merge bugfix/pre-1.2.13: release watch filter, album tooltip, copyable settings text
0b83c5a0 fix(css): keep pre, code, kbd, samp selectable globally
29e70de5 feat(discovery-v2): tooltip with full album title on AlbumCardLite
3903086f fix(release-watch): drop upcoming and non-streamable albums
04d5b451 docs(qbz-app): keep app comments frontend agnostic
2d7639c6 refactor(tauri): use qbz-app ui preference stores
d1082317 refactor(qbz-app): add ui preference stores
2b7b2312 Merge bugfix/pre-1.2.13: offline-tolerant CoreBridge + Continue Listening hydration
db390724 fix(discovery-v2): hydrate Continue Listening tracks with full metadata and context
a40b1b6c fix(core): make QbzCore::init tolerant to offline startup
7f38dbcc refactor(tauri): use qbz-app preference stores
61d5715a refactor(qbz-app): add preference stores
d48a1019 chore(deps): bump svelte to 5.55.7
917b2a8b fix(ui): avoid t shadow in mixtape detail
dac1b3a7 refactor(tauri): use qbz-app settings stores
800447d3 refactor(qbz-app): add settings stores
dc74db64 test(qbz-app): cover runtime state transitions
0877f4a5 refactor(tauri): use qbz-app runtime types
d16ef003 refactor(qbz-app): move runtime core
4f008cd2 refactor(qbz-app): scaffold app crate
1bb8d53d feat(graphics): NVIDIA Wayland compat toggle + watchdog fallback to CPU
db45051c chore(security): patch file-type 16.5.4 ASF infinite-loop (GHSA, alert 56)
2832213d Merge pull request #424 from vicrodh/dependabot/npm_and_yarn/pre-release/types/node-25.7.0
8b176a18 chore(deps): bump discord-rich-presence 0.2.5 → 1.1.0 with API adjustment
47c9c618 Merge pull request #432 from vicrodh/dependabot/cargo/src-tauri/pre-release/tower-http-0.6.8
875ad452 chore(deps-dev): bump @types/node from 22.19.18 to 25.7.0
7ee53599 Merge pull request #429 from vicrodh/dependabot/npm_and_yarn/pre-release/vite-8.0.12
b165ddb0 Merge pull request #430 from vicrodh/dependabot/npm_and_yarn/pre-release/sveltejs/vite-plugin-svelte-7.1.2
69fbb1df Merge pull request #426 from vicrodh/dependabot/npm_and_yarn/pre-release/tauri-apps/cli-2.11.1
11854a99 Merge pull request #427 from vicrodh/dependabot/npm_and_yarn/pre-release/tauri-apps/plugin-opener-2.5.4
0eae6a56 Merge pull request #425 from vicrodh/dependabot/cargo/src-tauri/pre-release/tokio-1.52.3
dc6725b1 Merge pull request #431 from vicrodh/dependabot/cargo/src-tauri/pre-release/filetime-0.2.29
91bfb056 Merge pull request #433 from vicrodh/dependabot/cargo/src-tauri/pre-release/tauri-plugin-single-instance-2.4.2
921f8e1a fix(session): resume Qobuz streams at saved position without losing seek (#315)
b2960e80 feat(graphics): smart GPU preference with boot watchdog and crash recovery
77754e16 feat(discovery): redesign Radio Station Lite cards to match iOS reference
cd035a34 feat(graphics): detect hybrid NVIDIA+iGPU and surface GPU + DE in Diagnostics
58807448 chore(deps): bump tauri-plugin-single-instance in /src-tauri
7ce112ba chore(deps): bump tower-http from 0.5.2 to 0.6.8 in /src-tauri
620a393b chore(deps): bump filetime from 0.2.27 to 0.2.29 in /src-tauri
b33ce39f chore(deps-dev): bump @sveltejs/vite-plugin-svelte from 7.0.0 to 7.1.2
9c83e116 chore(deps-dev): bump vite from 8.0.10 to 8.0.12
d53844c2 chore(deps): bump @tauri-apps/plugin-opener from 2.5.3 to 2.5.4
7e7f3e05 chore(deps-dev): bump @tauri-apps/cli from 2.10.1 to 2.11.1
060766e1 chore(deps): bump tokio from 1.52.2 to 1.52.3 in /src-tauri
b06580af feat(local-library): unify album quality badge + add source icon across surfaces
b5918e1c fix(library): recover Plex album format + bit-depth when codec is missing
0f5bff3a refactor(immersive): extract shared ImmersiveSongCard from 9 panel copies
5bc5122d fix(ephemeral): replace 1 << 48 with 2 ** 48 for ephemeral track id floor
7bb10472 fix(linebed): size artwork off the live measured column height
c715475d fix(linebed): use align-self: stretch on artwork instead of percentage height
783f7e41 fix(linebed): scale artwork to match track-meta column height
cb21653a fix(quality-badge): tighten framed-badge right gutter from 8px to 4px
72a1cbab fix(quality-badge): match framed-badge right gutter to Hi-Res SVG transparent space
978d5d68 feat(discovery): unified quality badge on album cards, Qobuz-style layout
d0349119 feat(settings): add dismiss + manual re-detect to the graphics recommendation banner
700d5ef2 feat(settings): surface graphics recommendation banner in the Graphics tab
11721594 refactor(settings): split Graphics out of Appearance into its own section
dc96c5ac feat(graphics): flip DMA-BUF default to opt-in
3061de21 chore(scripts): add dev-igpu-mode.sh for iGPU-only paint path testing
a2ab509f fix(toast): keep error text selectable for debug copy-paste
48b83d14 Merge pull request #423 from afonsojramos/rework-titlebar-drag-region
032021de feat(lyrics): drag region on sidebar header and selectable lyrics panel
e54c5de0 feat(views): drag regions on top-bars across remaining views
47f3a9f3 feat(views): drag regions on home, library, and favorites top-bars
897f6ac8 feat(detail-views): drag regions and selectable metadata blocks
eb57c70e feat(modals): mark info modals and legal notice selectable
418bf371 refactor(styles): drop now-redundant local user-select: none
a34e4185 feat(sidebar): drag window from aside top padding band on macos
69d7ee02 refactor(layout): remove macos overlay drag and main-content padding
5610861e refactor(titlebar): drag on spacer divs not full bar
c1c151ee refactor(css): user-select none on body with .selectable opt-in
f12b4e29 Merge branch 'feature/queue-persistence-315' into pre-release
d4745f84 feat(session): restore local on startup, let QConnect overwrite when it lands (#315)
267440f5 feat(session): round-trip streamable, parental_warning, source_item_id_hint (#315)
7284568c feat(session): persist queue on every mutation, not just track-change (#315)
6f8f6f43 Merge branch 'bugfix/qconnect-batch-pagination-419' into pre-release
6f05fd30 fix(qobuz): chunk get_tracks_batch into 50-ID windows (#419)
93f51d26 Merge pull request #422 from afonsojramos/document-stream-error-clear
8445f845 Merge pull request #421 from afonsojramos/close-other-filters-on-open
5865c166 docs(player): explain set_stream_error(false) message swallow
0daca399 Merge pull request #420 from afonsojramos/refactor/internal/macos-shared-mode-followup
9231ea2b fix(mixtapes): replace click-swallowing backdrop with document click-outside
b3b0fce5 fix(purchases): replace click-swallowing backdrop with document click-outside
3382a36e fix(mixtape-collection): replace click-swallowing backdrop with document click-outside
29598747 fix(collections): replace click-swallowing backdrop with document click-outside
ff11dea8 fix(label): drop tracks context menu backdrop in favor of click-outside listener
ddf0ed41 fix(artist-detail): consolidate section sort menus and drop tracks context backdrop
126dff12 fix(library): consolidate toolbar dropdowns into single openMenu state
7fccf9bd fix(favorites): consolidate toolbar and section dropdowns into single openMenu state
fc56fb4b feat(genre-filter): make isOpen bindable for parent dropdown coordination
65320134 perf(lyrics): drop karaoke gradient in non-immersive CPU mode
427e1b5b Merge branch 'feature/multiselect-completion-381' into pre-release
58a80b57 feat(favorites): select-all checkbox for track multiselect
2e2c7549 feat(local-library): finalize #381 multiselect with cross-context apagador
000f7c10 Merge pull request #418 from afonsojramos/fix-macos-lyrics-text-color
84dcd7eb perf(+page): guard lyrics state writes against subscriber churn
640dead3 fix(lyricsStore): claim rAF handle pre-tick and notify on every change
6166584e perf(playerStore): extrapolate audio time between backend position events
64511c4f feat(lyrics): segment active-line gradient by visual line via pretext
57f83079 feat(ui): toast on audio:init-failed event
a2518490 i18n(toast): add audioInitFailed translation key
6d91ec0e feat(audio): emit audio:init-failed Tauri event on backend init failure
04e17cc4 feat(player): expose user-readable stream_error_message channel
a7b40bf6 perf(player): cache CoreAudio nominal rate query for 750ms
83fb109b Merge pull request #416 from afonsojramos/debug-local-update-issue
da3c1e86 Merge pull request #410 from Vudgekek/fix/internal/macos-shared-mode
f5e00e0a Merge branch 'feature/cpu-mode-lite-panels' into pre-release
4b290308 feat(immersive): CPU-mode lite path + per-panel toggles + warning modal + badge unification
f528129f Merge branch 'feature/412-track-list-album-covers' into pre-release
145831fe feat(local-library): opt-in album artwork in tracks/folders list (#412)
9abc82b8 Merge bugfix/411-album-title-prefer-metadata into pre-release
bdf6ca30 fix(local-library): prefer album metadata over folder-derived title (#411)
cd799211 Merge branch 'feature/discovery-v2-scaffold' into pre-release
185d5abd fix(search): card width + quick-menu portal
5045096e fix(album-card-lite): contain: paint on cover-wrap forces clip
de30ede4 fix(album-card-lite): bound cover img to parent + delete dead legacy views
234af9d6 fix(search): align-items: flex-start + fixed wrapper width
36b6a2ee feat(purchases): migrate AlbumCard → AlbumCardLibraryLite
d5d8d29d feat(favorites): migrate AlbumCard → AlbumCardLite
d02e5d38 feat(views): migrate Award + AlbumDetail + ArtistsByLocation to AlbumCardLite
79cee870 feat(label): migrate AlbumCard → AlbumCardLite
550d75bb feat(search): migrate AlbumCard → AlbumCardLite
10e32aa0 feat(artist-detail): migrate AlbumCard → AlbumCardLite
447344c8 fix(local-library): plex id double-prefix + scrim z-index over buttons
9f417999 perf(plex): request 220x220 transcoded artwork for thumbnail contexts
4bc1b0fa fix(local-library): hover ring above img, scrollbar height + style match
87d16926 fix(local-library): visible scrollbars + list as default view
1aea33cf feat(local-library): Plex consolidation in chunked albums page query
2e58c889 fix(library-card): hover indicator uses inset box-shadow (not clipped)
9c2ce894 style(legacy-grid): match Discovery 32px gap in VirtualizedAlbumList
478582e2 style(library-card): hover outline + 220px card width to match Home
82014205 style(grid-pool): bump card gaps to 32px to match Home visual rhythm
1a5131ba style(library-card): drop solid theme buttons for white outline + shadow
55a139da style(library-card): align action buttons at bottom 44px (matches Home Lite)
694423c7 perf(library-card): drop hover entirely, theme-coloured persistent buttons
36ce669c perf(grid-pool): revert transform + contain, drop bufferRows 2→1
9003d93a perf(grid-pool): drop bufferRows 3→2 + skeleton placeholder
f494b1e6 perf(library-card): drop full-cover hover scrim, keep floating buttons
7d26522f perf(grid-pool): contain: layout paint on each slot
a3f1426c perf(grid-pool): position slots with transform instead of top/left
7508c3cd fix(local-library): fall back to legacy path when Plex is enabled
8f2cd965 feat(local-library): server-paginated chunked store for Albums grid
810a41b4 feat(local-library): recycling-pool virtualizer for Albums grid
3eb62a50 feat(discovery-v2): AlbumCardLibraryLite + migrate LocalLibrary grid
b2e88c47 refactor(settings): hard-disable chrome toggle without GPU, terser note
d1ad0fc8 feat(window): disable transparent chrome when hardware acceleration is off
884da638 chore(scripts): add dev-cpu-mode.sh for forced software rendering
b5db0241 fix(LyricsLines): make line-text block to expand background-clip paint area
f9a9d103 fix(LyricsLines): let EMA accept rAF-rate block measurements
c68b1e28 refactor(modals): migrate all 22 shared-modal callers to ModalLite
f4f8fcf9 fix(LyricsLines): use activeProgress as karaoke floor, lookahead as bonus
4de657e2 feat(LyricsLines): JS-driven karaoke with measured block-interval and lookahead
dcb13568 feat(discovery-v2): ModalLite — add withScrim + footerAlign + overlay backdrop
d729c68d feat(discovery-v2): spotlight rewrite + artist follow + loading skeleton
6d251001 refactor(LyricsLines): single-div template with class-toggle transitions
4d426a80 fix(lyrics): snap line progress to 1 when at least 99% through
959f6568 refactor(lyrics): notify listeners on every progress change
13d1d691 perf(lyrics): drive active-line updates via requestAnimationFrame
a5100a32 perf(page): memoize lyrics sidebar lines projection
9aacf1d7 feat(lyrics): parse LRC gap markers and use endMs for line bounds
556a345c feat(discovery-v2): RadioCardLite + Spotlight radio + Mixes resize + i18n fixes
5a8d3a20 feat(discovery-v2): For-You-exclusive sections (Spotlight, Rediscover, etc.)
fdd8de48 feat(discovery-v2): per-tab section preferences (Home / Editor's / For You)
704392d3 feat(runtime): conditional flicker fix gated by HW-accel state
da1b29b2 feat(graphics): default hardware_acceleration to 1 (was 0)
e9504efa perf(modals): gate 8 always-mounted modals with {#if}, drop blur shadow
e3cdc97a feat(discovery-v2): ModalLite — opacity-only animation, no scale transform
7696b1ee fix(discovery-v2): prioritize Album of the Week over other ribbons
86195fcb feat(discovery-v2): rephrase queue + offline actions, reorder
35520e93 feat(discovery-v2): playlist hover overlay + quick menu (refactored, not ported)
87423e92 feat(discovery-v2): ribbon to bottom, meta to top-left, actions raised
fdeb544c fix(lyrics): correct active-line color on macOS WebKit
2baa2e28 feat(discovery-v2): responsive Top Albums grid + Hi-Res only badge + meta tweak
9285a380 feat(discovery-v2): Top Albums rank counter + hover-play + kebab menu
b4e08ac5 feat(discovery-v2): wire add-to-playlist + songlink share to album quick menu
5d33db2d feat(discovery-v2): wire kebab to minimal action quick-menu
d9c9ec11 feat(discovery-v2): genre/year meta + slide-up actions + bottom positioning
871e550d feat(discovery-v2): Qobuz-style hover overlay with 3 action buttons
c1510201 feat(discovery-v2): cheap hover overlay on album cards
f9479ac8 feat(discovery-v2): Popular albums as compact 4×3 grid
3e4e04a4 feat(discovery-v2): playlist cover with object-fit:contain + dominant color bg
0c8cde75 feat(discovery-v2): compact track-row grid for "Continue Listening"
42f59033 feat(discovery-v2): quality badges + press / award ribbons on album cards
b9a902a8 chore(discovery-v2): per-section item count 6 → 18
29a6f386 perf(zoom): only register wheel listener while Ctrl/Meta is held
67118b21 perf(back-to-top): keep button mounted, swap mount/unmount for opacity
e477d0ce perf(modal): drop backdrop-filter blur on overlay
3fb9e554 feat(discovery-v2): customize modal + gear icon + bigger gaps
efa7b2b1 feat(discovery-v2): section preferences (default 7 enabled)
5b608e1d feat(discovery-v2): bump card size 180px → 220px (matches image.small)
fb987366 feat(discovery-v2): wire ideal-discography section
5ae5087f feat(discovery-v2): drag-to-paginate, fade transition, bump to 20 items
fa78f6af feat(discovery-v2): route artwork through cachedSrc
1ad08f2d feat(discovery-v2): pagination by DOM slice (no scroll containers)
368f0b42 Revert "feat(discovery-v2): horizontal-scroll sections (Cider pattern)"
612384ae feat(discovery-v2): horizontal-scroll sections (Cider pattern)
f6ac95ec feat(discovery-v2): highlight currently-playing track in continue listening
fcdd6021 feat(discovery-v2): wire genre filter button into toolbar
5c30ced8 feat(discovery-v2): gate scroll-area by active tab
e4040044 feat(discovery-v2): wire personalized + playlist + artist sections
aa526fb8 feat(discovery-v2): wire editorial sections from discover-index
b6d2c121 feat(discovery-v2): first section — new releases
373018a1 feat(discovery-v2): scaffold clean-room rebuild of home view
03af983a chore(scripts): add measure-cpu-window for structured perf sampling
3c6f5f90 perf(now-playing-bar): drop invisible backdrop-filter
efcdad55 perf(cached-image): drop forced compositing layer on every artwork
82b3217c perf(playlist-card): drop blur(30px) on artwork backdrop
9aa10e95 perf(album-card): drop hover marquee, decorative blurs, add content-visibility
6029e93f Revert "Merge pull request #407 from afonsojramos/fix-glued-top-animation"
43a35647 Merge branch 'bugfix/unavailable-track-row-dim' into pre-release
03d1142f fix(ui): dim unavailable tracks across all track list views
4fd85b5b fix(ci): suffix macOS .app.tar.gz bundles with arch to fix auto-updater
1edde8a2 fix(audio): rephrase CoreAudio rate-mismatch errors for end users
9e632386 fix(audio): retry CoreAudio shared-mode open on rate mismatch
c408ea73 test(player): cover compute_needs_new_stream decision rule
ac60113c refactor(player): extract StreamRecreateDecision helper
3b34a734 refactor(player): rename current_sample_rate to current_track_sample_rate
7bad704e fix(audio): fix macOS Shared Mode playback from changing speed
d44ee7c0 build(flake): bump qbz to v1.2.12
