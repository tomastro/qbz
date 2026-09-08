be63d6b3d chore(flatpak): point metainfo screenshots at the 1.2.5 commit
5f759514d chore(flatpak): refresh screenshots for 1.2.5
824b577dc fix(genre-filter): popup uses CSS max-height for smart positioning
604924064 chore(release): bump version to 1.2.5 "Accolade Watch"
372357497 fix(sidebar,about): smart positioning for playlist menus + platform label
7d1e5604d refactor(appearance): keep only the rounded corners on match-chrome
4c5019dff fix(appearance): prioritize visible edge over drop shadow
74d4d98e4 fix(appearance): draw the external drop shadow around rounded window
823e1c313 fix(appearance): smoother rounded corners under WebKitGTK
57e63e3ef feat(appearance): phase 2 of match-system-window-chrome (transparent)
8502c2fdb feat(appearance): match system window chrome (phase 1) + ribbon cleanup
62d167467 chore(release): hide qbzd from UI and CI until it ships
6a5cfc723 feat(titlebar): auto-detect KDE Plasma/Klassy theme for window controls
1cb8d2c30 feat(branding): refresh app icon, tray theming, and login branding
b227355af fix(nix): export LD_LIBRARY_PATH for libappindicator in devShell
c63c3ab16 chore(audio-watch): drop deprecated DeviceTrait::name fallback
887126237 chore(deps): bump rand to silence dependabot advisories
551c98659 fix(nix): export LIBCLANG_PATH in devShell for mupdf-sys bindgen
f7e51b980 fix(audio-watch): stop pre-flight device toast — stored name format never matched cpal enum
6094876aa fix(player): pass durationSecs on session-restore first play so seekbar advances
77d54f99d fix(qconnect): always persist local session to preserve track-level restore (#304)
b62a4b01f feat(audio): notify user when selected output device goes missing (issue #307)
7425d34e2 chore(assets): add laurel wreath SVG used by AwardView placeholder
d8fc85a07 fix(artist): dedup releases by id in convertPageArtist initial load
e6ed62657 fix(artist): replace gold QualityBadge with plain text to match AlbumView
d719fb679 feat(award): swap trophy icon for white laurel wreath on gold gradient
47963c0a4 fix(award+home): reload AwardView on awardId change; drop ribbon from Essential Discography
7c2f2941f feat(award): gold press ribbon + Other awards carousel on AwardView
c4c4bdf24 fix(album-awards): accept LegacyAwardDto's awardId/awardedAt field names
776e7846c fix(award-catalog): exhaustive /award/explore pagination with diagnostic logs
d75a8505b debug(album): log full /album/get payload; catalog can harvest award ids from album responses
b7f45b2d9 refactor(award-catalog): remove hardcoded editorial seed, rely on /award/explore
2dc9c143d feat(award-catalog): seed editorial Qobuz awards across locales + diacritic-insensitive lookup
6a5f9d090 feat(award): add See all link and dedicated AwardAlbumsView
854eaaa0a feat(award): resolve award id by name via /award/explore when /album/get omits it
946083822 fix(album-sidebar): keep award card always clickable; toast if id missing
037e18423 fix(award): normalize award.id as string end-to-end so /award/* calls stop sending 'null'
a27317471 feat(award): use /award/getAlbums for the grid + follow award as entity
e52e3e1fd fix(award): loosen AwardPageData and AwardMagazine for inconsistent Qobuz shapes
8730c891c feat(award): dedicated AwardView with header matching Label/Album conventions
ee2f413ec fix(album): use dedicated tolerant AlbumAward struct instead of mutating PageArtistAward
fe3213ece feat(award): backend V2 commands for /award/page and /award/getAlbums
f0b4499da fix(album): accept awarded_at as integer or string on PageArtistAward
7bf997284 feat(album-view): right sidebar with label and full awards stack
ef9cab64b feat(ribbons): extend album ribbon to press accolades (last award wins)
ebeb86d03 feat(release-watch): dedicated tabbed view mirroring Qobuz mobile
060395885 fix(release-watch): backfill album.artist from artists[0] on getNewReleases
de056abd8 fix(home): move album ribbon to bottom-left and let overlay cover it on hover
b738c2750 fix(home): plumb album awards through backend + invalidate stale home cache
89c9ee102 feat(home): add editorial ribbons for Qobuzissime and Album of the Week
dcea5fe6b fix(labels): drop label-card background hover so follow-btn hover is visible
8ca2accb9 fix(labels): replace pill with 6px rounded rectangle for label follow btn
d98c89387 fix(foryou): wait for topArtists/recentAlbums before loading dependent rails
8b0bd820d feat(labels): replace heart overlay with pill Follow button below card
79e0f93b8 fix(home): persist Release Watch in home cache so it renders on revisits
ebe999c43 feat(favorites): add Labels tab to Favorites view
50f5fd777 feat(labels): add follow heart overlay to more-labels cards in LabelView
a9a9ba505 feat(labels): follow / unfollow labels (mirrors Follow Artist)
40ba41448 feat(discover): move Release Watch to below Your Mixes + add home.releaseWatch i18n key
37c64d252 fix(api): project /favorite/getNewReleases envelope to SearchResultsPage
ce375008a fix(api): Release Watch lives at /favorite/getNewReleases?type=artists
541c3613c chore(api): promote release-watch API logs to info for diagnosability
b0e724d42 fix(api): call /albums/releaseWatch as REST, not signed RPC
60e0861e1 refactor(discover): read Release Watch from /discover/index response
9945e93e0 fix(api): tolerate unknown release watch response shape
ea216adf0 feat(discover): add Release Watch section to Home and For You
808b9db0d fix(offline): real offline mode — snapshot streaming, block network, diag logs (#279)
722696df3 fix(window): clamp persisted size to fit the largest available monitor
10bc0eb66 feat(tray): use ksni on Linux for working left-click toggle (#310)
247a565ac fix(ui): remove silk animations in ForYouTab mix cards
40f1aa344 fix(ui): remove silk animations from Your Mixes covers
0898340aa fix(ui): keep QconnectBadge compact at full bar height
014af6fbf fix(build): stop infinite tauri dev rebuild loop on auto-updater.json
568dc31df fix(ui): collapse right-section to hamburger at narrow widths (#303)
160dc7a52 feat(ui): compact variants for QualityBadge and QconnectBadge
cc21a82d6 fix(ui): refresh audio output badges when playback starts
33761c646 feat(ui): surface BitPerfectMode in QualityBadge (#288)
8311cef82 feat(player): expose BitPerfectMode in PlaybackEvent (#288)
9c2896060 fix(audio): clearer error when CPAL cannot open an enumerated device (#288)
cba5b76bb fix(audio): fallback to plughw instead of CPAL when DAC rate unsupported (#288)
d7c811995 fix(audio): downgrade track quality when DAC rate unsupported (#288)
4bb79fdd7 ci(updater): pass --no-default-features in sandboxed builds
031da1792 feat(updater): gate auto-updater behind Cargo feature `updater`
1777760ae chore(updater): set real minisign public key
347a8c101 feat(updater): wire auto-update handlers in app and settings
137204099 feat(updater): extend update modals with auto-update option
10b5d9585 feat(updater): add performAutoUpdate and eligibility to store/service
46f8693f1 feat(updater): add UpdateProgressModal with i18n
f664ce327 i18n(updater): add autoUpdate keys and download actions
a98d6d21b chore(deps): add @tauri-apps/plugin-updater and plugin-process
2f5a6aadd ci(updater): sign artifacts and consolidate updater manifest
96cfb9395 feat(updater): add tauri-plugin-updater backend integration
6c66961ca chore(deps-dev): bump @sveltejs/kit from 2.57.0 to 2.57.1
64a42fd15 fix(qbzd): use SetState.next_track for auto-advance + periodic state reports
4112cc77b fix(qbzd): materialize QConnect remote queue for auto-advance
518219793 fix(qbzd): prevent playback restart from server state echo
fc83ad6cc chore(qbzd): bump version to 0.1.7
3df007fe9 fix(qbzd): download and play track on QConnect renderer SetState
79a86278f fix(qbzd): complete device_info with device_uuid + capabilities
ffa59b339 fix(qbzd): rewrite QConnect to exact desktop pattern
bea94871e fix(qbzd): immediate renderer join in bootstrap, don't wait for SESSION_STATE
186e79d22 fix(qbzd): QConnect renderer join via SessionManagementEvent sink
fb10b46ac fix(qbzd): early return on remote play, zero local side effects
82ea7517c chore(qbzd): own version 0.1.6, decoupled from workspace
c2e8285df fix(qbzd): prevent orchestrator race on play-track + QConnect debug logging
ca4480239 feat(qbzd): guard all stores/services for remote mode override
b49fce766 feat(qbzd): guard all stores/services for remote mode override
98286c612 fix(qbzd): skip QConnect handoff when controlling daemon via HTTP
39142aad3 feat(qbzd): proper playback orchestrator with gapless + repeat modes
30bc325f6 feat(qbzd): auto-advance queue + play-album endpoint
3a01330b0 fix(ui): banner layout uses CSS variable, fix i18n key, responsive height
72254ac44 fix(qbzd): remove token requirement from remote API client
6cf396aa8 fix(qbzd): QConnect bootstrap clears pending between commands
e9fe6b624 fix(qbzd): QConnect form-encoded createToken + mDNS hostname + response parsing
457c62831 feat(qbzd): implement play-track endpoint (download + play)
ca9ef961c fix(qbzd): full QConnect protocol + createToken Content-Length fix
264302fd8 feat(qbzd): implement all wizard sections (cache, integrations, qconnect)
c63799309 fix(ci): use native ARM64 runner instead of QEMU for aarch64 build
7a9f8407a fix(qbzd): file-based token fallback when keyring unavailable
fd1a4fade fix(qbzd): route ALL playback and queue calls through commandRouter
e4b6024ad fix(ui): remote indicator sits above player bar, not inside it
a9282e209 feat(qbzd): manual daemon connect + persistence + headless login
c21e87e1c fix(qbzd): headless-compatible login with LAN IP detection
aaa183ce3 feat(ui): add Select All checkbox to multi-select mode in all track views
280252791 chore(deps): update jsdom 29.0.2, notify-rust 4.14.0
dd322a180 fix(ci): use QEMU native build for aarch64 instead of cross-compile
1cba3ec18 fix(ci): force all apt sources to amd64, arm64 from ports only
22a34b4f3 fix(ci): target glibc 2.35 instead of 2.31
7a522623c fix(ci): add libdbus-1-dev, fix arm64 apt sources, add strip
c3ce3c666 fix(ci): drop openssl dependency, use rustls-only for all crates
d042d2162 fix(ci): add libssl-dev + pkg-config to qbzd build
8cee1c60b ci(qbzd): fix workflow paths, add cache, track Cargo.lock
586d27bf4 feat(qbzd): integrate daemon discovery into CastPicker
eade14334 feat(qbzd): TUI setup wizard with ratatui (Phase 1.5)
632b13495 feat(qbzd): full metadata scan using MetadataExtractor
90769c053 ci: add qbzd build workflow (x86_64 + aarch64, glibc 2.31)
e5539132d feat(qbzd): QConnect headless integration
b3e5be39e feat(qbzd): MPRIS metadata from CoreEvent::TrackStarted
a5efd6ae1 feat(qbzd): setup subcommand placeholder + config save helper
e0301b6a2 feat(qbzd): MPRIS playback updates + status subcommand
f9d6e551e feat(qbzd): LAN CORS, headless MPRIS, security model
892560dff feat(qbzd): LAN-only access restriction, no token auth
aacdeabe0 feat(qbzd): login subcommand, library scan, service files (68 endpoints)
42a8ff566 feat(qbzd): library management + integrations API (67 endpoints)
b524d6231 feat(qbzd): API tiers 2-3 — discover, playlists, favorites, labels, system (55 endpoints)
8c947c02b feat(qbzd): audio settings API + remote settings routing (Phase 5e)
8fc611178 feat(qbzd): mDNS discovery + registration for LAN pairing
8dac18526 feat(qbzd): remote mode indicator in NowPlayingBar
e3c1e471c feat(qbzd): bifurcate playback and queue stores for remote mode
0d94f2147 feat(qbzd): add remote playback target store and API services
3b8555992 feat(qbzd): HTTP API tier 1 - playback, queue, search, catalog (Phase 4)
ac1778726 feat(qbzd): unified event bus + SSE endpoint (Phase 3)
6ee123c2a feat(qbzd): add session lifecycle port (Phase 2)
e6bacb750 chore(deps): bump tauri-plugin-dialog from 2.6.0 to 2.7.0 in /src-tauri
608f9508e chore(deps): bump rodio from 0.22.1 to 0.22.2 in /src-tauri
80886b894 chore(deps): bump @tauri-apps/plugin-dialog from 2.6.0 to 2.7.0
6d40aacfa chore(deps-dev): bump vite from 8.0.5 to 8.0.8
b2527d829 chore(deps): bump tauri-plugin-deep-link in /src-tauri
e2a4e8307 chore(deps-dev): bump @sveltejs/kit from 2.55.0 to 2.57.0
2817ed6fb chore(deps-dev): bump svelte from 5.55.1 to 5.55.2
2916dee42 feat(qbzd): wire QbzCore, Player, AudioCache, keyring (Phase 1)
b0ed46766 feat(qbzd): scaffold headless daemon binary (Phase 0)
242b27bb1 refactor: clean up commands_v2/mod.rs
77f12075d refactor: extract commands_v2/diagnostics.rs
8fa11a431 refactor: extract commands_v2/discovery.rs
b0ae32896 refactor: extract commands_v2/image_cache.rs
709c399e3 refactor: extract commands_v2/session.rs
6f1a1c8a2 refactor: extract commands_v2/settings.rs
93154690e refactor: extract commands_v2/auth.rs
45ab6c34b refactor: extract commands_v2/catalog.rs
5f4cc58d9 refactor: extract commands_v2/playlists.rs
c746303f2 refactor: extract commands_v2/audio.rs
98f17019d refactor: extract commands_v2/favorites.rs
0303e40f3 refactor: extract commands_v2/search.rs
6c6199a76 refactor: extract commands_v2/queue.rs
02bae5b60 refactor: extract commands_v2/runtime.rs
555139a6e refactor: extract commands_v2/playback.rs
f4c874fda refactor: extract commands_v2/link_resolver.rs
d667c3e13 refactor: extract commands_v2/integrations.rs
82e1b337a refactor: extract commands_v2/helpers.rs
d8dbcf7bd refactor: extract commands_v2/library.rs
ccce56458 refactor: extract commands_v2/legacy_compat.rs
6cc75d091 refactor: convert commands_v2.rs to module directory
c56bd12e4 refactor(gapless): flatten local library check from 4 nested ifs
94949a7f5 fix(qconnect): persist custom device name across restarts
b6035cb3a perf(prefetch): increase cache depth to 5 tracks, 2 concurrent
438cc1885 perf(cmaf): parallel segment downloads for prefetch cache
5aa0df1cb feat(api): sign all remaining Qobuz API endpoints
e62b160d5 feat(login): add cancel buttons and captcha hint to OAuth flows
6efdca0ea feat(oauth): add cancel commands for both OAuth flows
814bf8b96 fix(oauth): detect WebView window close to unblock login wait
dbc97d6bd feat(search): add request signing to all search endpoints
fb7c2a440 docs: add NixOS installation instructions, update snap note
59b34132c fix(lyrics): use theme accent color for active line highlight
42b429e86 fix(ui): enable multi-select drag in all views
a7cdcd6f3 feat(ui): enable track drag & drop in artist and search views
e71d634cd fix(ui): show artist and album in track drag ghost
73b0da44a fix(ui): use compact drag ghost for track drag & drop
ae8990f4a feat(ui): drag & drop tracks onto sidebar playlists
7faa4e467 fix(audio): default alsa_plugin to Hw when switching to ALSA backend
b5bb2bbe0 fix(player): lock volume slider in immersive player when ALSA Direct hw active
acd283a1a feat(snap): restore mpris slot after snapcraft approval
e2fd2a11b feat(snap): restore mpris slot after snapcraft approval
20440c611 Revert "feat(snap): restore mpris slot after snapcraft approval"
3c104ce22 feat(snap): restore mpris slot after snapcraft approval
6624b7057 chore(snap): add exclusive audio note to description
c8a3a648f chore(alsa): lower device enumeration logs to debug, add 5th retry
426a4d0db fix(player): stop engine before dropping ALSA stream on format change
17919ccca chore: bump flake.nix to 1.2.4
c85fff490 Delay firstId variable declaration until needed
14f7d4d7d Removed change to library playback caching behaviour not necessary for fix
130a127c8 Get repeat mode from bridge instead of app_state to avoid using deprecated type and having to set repeat mode in two places
4c786c86f Moved logic to repeat current track gapless from backend to frontent
151351a2b Fail silently if track not found in local library
3feaed5e4 Removed comment noise
28bcfeba7 Fix loop one mode when using gapless playback
f99dbe9fd Try local library for gapless playback if not in any cache