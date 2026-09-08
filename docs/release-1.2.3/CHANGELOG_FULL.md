e14e0a14 fix(logout): stop playback before deactivating session
241dcf08 feat(security): store OAuth token in system keyring
c37f3d75 feat(security): enable system keyring for credential storage
54139ad4 fix(offline): allow playback controls without API client init
a7da0634 chore(deps-dev): bump svelte 5.55.0 -> 5.55.1
3eab5b72 chore(deps-dev): bump jsdom 27.4.0 -> 29.0.1
2c0f42b8 chore(deps): bump dirs 5 -> 6 across all crates
a98d3d95 chore(deps): bump vite 8.0.3 -> 8.0.5 (security)
f86bc960 merge: CMAF segmented streaming pipeline
3df84f81 fix(quality): request track's actual quality instead of always UltraHiRes
6103044a fix(quality): parse track metadata quality instead of defaulting to UltraHiRes
8b9abdea Revert "fix(cache): don't discard 24-bit cache for sample rate mismatch"
49fc2ae4 fix(cache): don't discard 24-bit cache for sample rate mismatch
63120010 fix(download): migrate remaining legacy paths to CMAF-first
dcde40b0 fix(prefetch): use CMAF pipeline for prefetch downloads
01a12e21 feat(streaming): CMAF streaming playback with immediate start
bf555ab1 feat(streaming): CMAF segmented download pipeline
0265cb99 feat(qobuz): add session/start and file/url CMAF endpoints
8133931d feat(models): add SessionStartResponse and TrackFileUrl types
b3a90697 feat(cmaf): add CMAF segment parser
794dd9f2 feat(cmaf): add qbz-cmaf crate with crypto module
9a95a54e fix(queue): prevent ghost next_track after context switch
def821bd fix(download): add User-Agent header and remove stale timeout
44390700 fix(alsa): progressive backoff retry on device busy during track skip
9f2c423c fix(offline-cache): prevent HTTP/2 connection pool poisoning
09fcb5eb chore(deps): upgrade alsa crate 0.9 -> 0.10
96967e81 chore: regenerate lockfiles for tauri-plugin-os dependency
5a4f014b Merge pull request #265 from afonsojramos/gate-flatpak-snap-macos
9cbbbad6 Merge pull request #272 from afonsojramos/investigate-corrupted-app
c1793597 Merge pull request #247 from GwendalBeaumont/refactor/external/i18n-components
34daf20f style(immersive): match Static and AlbumReactive cover sizing to other views
e5e1516b style(immersive): soften quality badge backdrop to keep glassy look
b431142d fix(coverflow): restore original spacing, rely on z-index for layering
65792b3a fix: sync package-lock.json with package.json
e4462dfe fix(macos): ad-hoc sign app bundle to fix Gatekeeper rejection
a73cfee9 fix(coverflow): restore center image, add quality badge backdrop
0096c57b fix(coverflow): eliminate side cover overlap with center
71680686 fix(coverflow): larger covers, fix overlap, remove broken animation
53a02f4e style(immersive): increase font sizes +4pt in split panels
ade8d903 style(immersive): increase artwork size and reduce wasted padding
2de1aa5d Revert "style(immersive): restructure split layout to fill available space"
db27a361 style(immersive): restructure split layout to fill available space
59dc9c9d style(immersive): larger artwork, Montserrat font, hidden scrollbar
a3f63f21 style(immersive): improve split lyrics layout for better space usage
d709d75f Merge branch 'pre-release' into refactor/external/i18n-components
787196db fix: keep exclusive mode and DAC passthrough visible on all platforms
86d5286a fix: add Windows case to AboutModal platform label
2e5b9368 fix: add windows to Platform type
80a57723 fix(i18n): critical issues
85bef48a fix: use linux-only gates instead of not-macos for platform-specific settings
94bd222c fix: gate Linux-only settings sections on macOS
b7128418 refactor: migrate platform detection to Tauri OS plugin utility
e089417a feat: create platform utility using Tauri OS plugin
5d115277 feat: add @tauri-apps/plugin-os for platform detection
8817c089 feat(i18n): add new roles
4dce62a7 feat(i18n): add translations to roles
bfee0ba8 feat(i18n): add missing translations
2ad5f0e2 feat(i18n): strings for HomeSettingsModal
a9ce5519 feat(i18n): translation strings in HomeSettingsModal
bbecb346 feat(i18n): add missing strings
e7dc3811 feat(i18n): spanish translation strings
8943320f feat(i18n): portuguese translation strings
e0506cd9 feat(i18n): english translation strings
2e38db22 feat(i18n): german translation strings
76853592 feat(i18n): french translation strings
7c7aba5f feat(i18n): translation strings to TrackMenu
6e0f4abd feat(i18n): add fr translations
117227ea feat(i18n): add pt translations
0ea54b7e feat(i18n): add es translations
a22a87e7 feat(i18n): add en translations
023b424a feat(i18n): add de translations
e680c92b chore(i18n): format keys
57993864 fix(i18n): wrong translation keys
3e53d700 feat(i18n): missing translation strings
b18ccd4f feat(i18n): translation strings in Playlists views
363506f1 feat(i18n): translation strings in SuggestionsPanel
e993638c feat(i18n): translation strings in TrackInfoPanel
6805f9b3 feat(i18n): translation strings in SpectralRibbon
430b9343 feat(i18n): translation strings in HistoryPanel
078ea0e1 fix(i18n): update typo
39e8a0c6 feat(i18n): add Explicit translation string
d2852324 feat(i18n): translation strings in ImmersiveHeader
6caa6a1f feat(i18n): translation strings in Modal
a2c187b7 feat(i18n): translation strings in LinkResolverModal
eb324a91 feat(i18n): translation strings in HomeSettingsModal
fcfcd97b feat(i18n): translation strings in FolderEditModal
aa4ae71b feat(i18n): translation strings in HeroSection
679c749b feat(i18n): translation strings in FocusMode
2ca635e1 feat(i18n): translation strings in FavoritePlaylistCard
830fd2d9 feat(i18n): translation strings in ExpandedPlayer
61bc08a2 feat(i18n): translation strings in Dropdown
e3b29d91 feat(i18n): translation strings in DeviceDropdown
865da979 feat(i18n): translation strings in BulkActionBar
81aeba3f feat(i18n): translation strings in AlbumCreditsModal
bf47ad7b feat(i18n): add english translation strings
820b5131 feat(i18n): add translation strings to BitPerfectAppSelector
2677da68 feat(i18n): add translation strings to WhatsNewModal
19959b84 feat(i18n): add translation strings to UpdateReminderModal
c403b78f feat(i18n): add translation strings to UpdateCheckResultModal
60f0a780 feat(i18n): add translation strings to UpdateAvailableModal
4b0cd5ac feat(i18n): add translation strings to SnapWelcomeModal
01b3507e feat(i18n): add translation strings to FlatpakWelcomeModal
96bc244c feat(i18n): add translation strings to VolumeSlider
9f185cf5 feat(i18n): adjust translation strings in TrackInfoPanel
73346706 feat(i18n): add translation strings to SuggestionsPanel
3bd590e2 feat(i18n): add translation strings to StaticPanel
20d0e752 feat(i18n): adjust translation strings in QueuePanel
6792bc7d feat(i18n): adjust translation strings in LyricsPanel
91d941b5 feat(i18n): add translation strings to CoverflowPanel
a51f72b1 feat(i18n): add translation strings to AlbumReactivePanel
393c88f5 feat(i18n): add translation strings to PlayerControlsCompact
2c074bf4 feat(i18n): add translation strings to ImmersiveHeader
ba8e85c5 feat(i18n): add translation strings to ImmersiveControls
b02a5982 feat(i18n): add translation strings to CastPicker
17b073bb feat(i18n): add translation strings to BulkActionBar
7b2d6b20 feat(i18n): add translation strings to AlbumMenu
43890fb5 feat(i18n): add translation strings to AlbumCreditsModel
e0f4c151 feat(i18n): add translation strings to AlbumCard
