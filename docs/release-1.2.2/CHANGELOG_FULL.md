ea2280f8 chore: update codename to Bye bye plain auth
ee019851 chore: bump version to 1.2.2
e66117fb feat(auth): remove basic auth login, OAuth only
adfc3900 fix(notifications): use released notify-rust 4.13.0 instead of fork
7a1890ad fix(audio): use released coreaudio-rs 0.14.1 instead of fork
b1000cb9 fix(macos): gate Linux graphics CLI flags and crash handler with cfg
34a18093 fix: restore albumId/artistId on session resume for player bar navigation
3ac7073b feat(qobuz): migrate login_with_token to V2 crates
211f45a5 fix(favorites): bulk make offline now actually downloads tracks
ba3ccf9a feat(settings): hide tray sub-settings on macOS and use menu bar terminology
7edffcf5 feat(i18n): add macOS-specific menu bar tray keys for all locales
84b1f1a5 docs: remove Apple Silicon only note from macOS section
833cc2b3 fix: boost immersive drag region z-index on macOS
8283bec6 feat: add Cmd+, keyboard shortcut to open settings
35d0ff39 chore: remove unused github.svg (replaced by simple-icons in PR #237)
b8af1763 feat(macos): add album artwork to notifications via image_path
a4a591e7 feat(macos): switch to notify-rust fork with image_path support
684dd558 fix(macos): scope title bar padding to non-home views only
9823dfb6 fix(macos): enable x86_64 cross-compilation
071865e7 fix: replace removed Github icon with simple-icons
cd2472f5 fix(macos): add top padding to clear native overlay title bar
b6ad1926 fix(macos): hide Linux-only settings on macOS
47835305 fix(macos): gate idle_inhibit module for Linux only
1e019bec fix: resolve correct artist and country for MusicBrainz metadata
1953774d fix: resolve birth country from area hierarchy instead of MB country field
305cffa0 fix: don't append country to birth location display
f702622e fix: correct artist birth/formation location display order
d19c142d fix: switch log upload to paste.rs (dpaste.org returns 403/405)
527d63ca fix: migrate log upload from 0x0.st to dpaste.org
a87e35d1 fix: restore MPRIS metadata and playback context on session resume
6acd8438 feat: add streaming buffer progress indicator to seekbar
18403bba fix: single ESC exits both immersive overlay and window fullscreen
b315b23b fix: F11 fullscreen toggle and ESC exit work globally
759149f3 feat(favorites): add bulk make-offline action to track selection bar
93374080 feat(titlebar): add Playlists to favorites dropdown in titlebar nav
b00f294e feat(sidebar): add Playlists to favorites quick access menu
080b916c feat: prevent system sleep during playback via XDG portal
2900fb88 feat: add log sanitization for sensitive IDs and UUIDs
4e526b1a chore(deps): upgrade vite 6->8, vite-plugin-svelte 5->7, kit 2.55
a99d558e fix: update lofty 0.18 API calls to lofty 0.23
166d3467 fix: update axum 0.7 API calls to axum 0.8
a2fb4451 feat(macos): add deep link support for qobuzapp:// URLs
66a0c8f0 fix: update rand 0.8 API calls to rand 0.10
00cf8cf4 refactor(icons): rename all deprecated lucide-svelte icons to v1 names
6bf397e0 feat(macos): auto-select SystemDefault backend on non-Linux
4c15a432 feat(macos): enhance CpalDefaultBackend with device capabilities
c51f07fe feat(macos): add CoreAudio device probing and sample rate switching
84c9b693 style: fix indentation and missing semicolon from PR #230
38258ec9 fix(i18n): post-merge corrections for PR #226
b6cebe26 feat(i18n): add translation strings to AboutModal
0549a701 feat(i18n): add french translation strings
67b8bf3c feat(i18n): add spanish translation strings
bc126341 feat(i18n): add english translation strings
881df48f feat(i18n): add translation strings to SettingsView
d97e8f86 feat(i18n): add translation strings to LocalLibraryView
28cf84a4 feat(i18n): translation strings in Sidebar
64061176 feat(i18n): translation strings in SearchView
38efc967 fix(i18n): middot in TopQView
935cf34c fix(i18n): date formatting in PurchasesView
34c6e413 fix(i18n): date formatting in PurchaseAlbumDetailView
0f0dfda1 fix(i18n): date formatting in BlacklistManagerView
17babe48 fix(i18n): date formatting in ArtistDetailView
40801dc7 feat(i18n): add translation strings to PlaylistmanagerView
fafd5473 Fix scroll position not being saved when navigating without scrolling
b7068f3d feat(i18n): add translation strings to MusicianPageView
95daf91d feat(i18n): add translation strings to LoginView
82d81352 feat(i18n): add translation strings to LabelView
868a6bdf fix(i18n): CD quality on HomeView
55b7d97a feat(i18n): add translation strings to ForYouTab
656866cd feat(i18n): add translation strings to FavQView
b2bd4a2a feat(i18n): add translation strings to DynamicSuggestView
37de089c feat(i18n): add translation strings to DiscoverBrowseView
eb7a6a9a feat(i18n): add translation strings to BlacklistManagerView
55bc56ae fix(i18n): title sort on artist master-detail view
46943be4 Fix scroll restoration for virtualized containers
b879a8f9 feat(i18n): adjustments on ArtistsByLocationView
cddb98ed feat(i18n): add english translations
d6a4b11a feat(i18n): add french translations
c0a29020 feat(i18n): add strings to FavoritesView
2e450fec feat(i18n): adjustments on artist detailed view
5609cc1c feat(i18n): album scrolling prev and next strings
18f99c7b feat(i18n): localization for CD Quality on quality badge
e4b98223 feat(i18n): localized release date on album credits
d6eedb7d feat(i18n): add language strings for artist page
0082f474 feat(i18n): add translation strings for CD Quality
b935b836 feat(i18n): add localization to album release dates
4a7cc604 feat(i18n): add translation strings in ArtistDetailView
1c51eac5 docs: add contributors section to README, remove inline credits
e34212a5 docs: add macOS experimental section to README
88c352f6 fix(gentoo): use libayatana-appindicator instead of libappindicator
