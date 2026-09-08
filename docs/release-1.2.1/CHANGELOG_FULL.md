5d9d956f Merge pull request #211 from vicrodh/dependabot/cargo/src-tauri/rustls-webpki-0.103.10
3dd09ea5 chore(deps): bump aws-lc-rs 1.16.1 -> 1.16.2, aws-lc-sys 0.38.0 -> 0.39.0
543e6cd1 fix(degraded): reduce clear timeout from 10 to 5 minutes
904502c5 feat(ui): add degraded service indicator for Qobuz server issues
cf2df33b feat(offline): add manual network check button
a8910a66 feat(prefetch): use exponential backoff for cache pre-loading
ead15162 feat(settings): add quality fallback behavior dropdown
e966f35b feat(ui): wire QualityFallbackModal into global layout
3559cede feat(playback): handle QualityExhausted with user preference
a9024344 feat(ui): add QualityFallbackModal component
283302f6 feat(i18n): add quality fallback modal and settings translations
9e53095b chore(deps): bump rustls-webpki from 0.103.9 to 0.103.10 in /src-tauri
b08e5ecf feat(audio): exponential backoff retry with quality fallback
94518e56 feat(audio): add v2 commands for quality fallback behavior
038f8468 feat(audio): add quality_fallback_behavior to audio settings
5ad11796 ci: add platform triage workflow for issue auto-assign
00d6d02c feat(issues): add platform dropdown to all issue templates
ff16767f feat(about): add afonsojramos and GwendalBeaumont to contributors
da32afff Merge pull request #208 from GwendalBeaumont/refactor/external/i18n-interpolation
ff234c37 refactor: add spanish interpolation strings
6f1424ce refactor: add french interpolation strings
aa604340 refactor: add english interpolation strings
1eb886da refactor: add interpolation to playlist view
e1147239 Merge pull request #1 from GwendalBeaumont/chore/external/update-fr-translation
235bd08e fix(qconnect): tolerate missing queue_version major/minor fields
39e5867a fix: proxy all images through reqwest to bypass WebKit TLS (#163)
e6ea2b48 Merge pull request #198 from vicrodh/dependabot/cargo/src-tauri/rustls-webpki-0.103.10
a25bcb0e fix(ci): macOS build aarch64-only, drop x86_64 cross-compile
a347b39e fix(ci): use macos-latest for both arch targets
84b3abc6 Revert "ci: add macOS release workflow (unsigned DMG)"
0ce4cc7b ci: add macOS release workflow (unsigned DMG)
052de7fb ci: add macOS release workflow (unsigned DMG)
9f7304b3 Merge branch 'pr-181' into pre-release
badbbf29 fix: ESC in immersive now exits fullscreen too (#202)
45b3390c Merge branch 'pr-205' into pre-release
d27ab665 fix: SystemTooltip wraps long text instead of overflowing (#166)
b826f88f feat(hifi-wizard): non-systemd support and verify step fix
9a1b88e5 fix: prevent image flicker on WebKitGTK 2.50+ (compositing layer fix)
499f188b fix: hybrid Intel+NVIDIA treated as NVIDIA-only for compositing
d9ea5f96 fix: make diagnostics panel on-request instead of always visible
7f952b11 feat: add system diagnostics panel to Developer Mode
be900ab6 feat: add v2_get_runtime_diagnostics command
0265f32a docs: add 3-phase plan for diagnostics, NVIDIA defaults, setup wizard
a472930c fix: respect saved graphics settings at startup
1a510415 perf: batch download status checks on Home view
38f4d20c refactor(SearchView): replace hard-coded strings
27b457de chore: update french translation
3366c2e8 ci(gentoo): use APT_REPO_TOKEN instead of OVERLAY_PAT
346db378 ci: add release-gentoo workflow for overlay auto-update
72b794a8 docs: add Gentoo overlay installation to README
c3accf7e fix: allow network folders in manual offline mode
eeb15564 fix: initialize all user stores in offline session activation
86520df9 feat: make offline notice banner dismissible in Local Library
8316e1f4 fix: logout spinner hang and last_user_id preservation
fb74f318 fix: restore offline mode — use last known user profile
c7663578 fix: detect BundleExtractionFailed in catch block for offline mode
2c0c46d2 chore(deps): bump rustls-webpki from 0.103.9 to 0.103.10 in /src-tauri
04161985 fix: retry download with effective quality after fallback (#196)
dde7c192 fix: allow volume control when QConnect peer renderer is active on ALSA hw:
a4782c64 fix(qconnect): refresh session snapshot when badge popup opens
f1f084c9 fix: use ~/.local/share/icons for Flatpak tray icon temp path
8b85e0ba fix: add libclang-dev to snap build-packages for mupdf-sys bindgen
b73a910f fix: add unzip to snap build-packages for mupdf-sys ODT extraction
70c27f37 fix(macos): log when DAC sample rate detection is skipped
ce332705 refactor: gate KWin functions with cfg(target_os = "linux")
379ba28c refactor: simplify should_use_main_window_transparency
6422ae8f refactor: cache isMacOS at module scope in titleBarStore
6017cc70 fix(macos): gate drag region to macOS only
7dca9620 fix(macos): merge duplicate platform detection and remove non-passive wheel listener
ebe555b7 fix(macos): make notification fire-and-forget
dad062d6 fix(audio): add docs and fix param names for non-Linux stubs
f854e096 feat(audio): add SystemDefault variant to AudioBackendType
77888fb3 fix: suppress unused_mut warning for cross-platform builder
08367fe9 fix: convert 16-bit PNG icons to 8-bit for Tauri compatibility
00147dc7 feat(macos): adapt frontend for native title bar
caffd7b5 feat(macos): enable private API and set minimum system version
3809a5cc feat(macos): add notify-rust dependency for desktop notifications
dbfc5e3f feat(macos): gate Linux-only commands and configure macOS window
9bcf2413 feat(macos): gate network module and fix remaining souvlaki error formatting
5ed549d4 feat(macos): gate Linux-only audio backends with cfg
