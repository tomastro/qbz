# QBZ 1.1.15 Full Changelog

This document details all changes introduced in version 1.1.15 relative to the v1.1.14 tag.

**Summary:** ~200 commits across all branches merged into pre-release; 19 files changed in the multi-select/navigation work alone.

---

## New Features

### Purchases view

- New `PurchasesView` component browses the authenticated user's Qobuz purchases via the V2 API
- Local download registry stored in a dedicated SQLite table; tracks which purchases have been downloaded and to which local path
- V2 backend commands: `v2_get_purchases`, `v2_get_downloaded_purchases`, `v2_mark_purchase_downloaded`
- Downloads organized into quality subfolders to match the Qobuz file naming convention
- Mock data removed; real API integration only

### Your Mixes

- New **Your Mixes** card section on the Home view
- Four mix types surfaced: **Daily Q** (`DynamicSuggestView`), **Weekly Q** (`WeeklySuggestView`), **Fav Q** (`FavQView`), **Top Q** (`TopQView`)
- Each view: full track list with artwork, quality badges, shuffle, search/filter, and now multi-select
- Video/animated assets for mix cover placeholders

### Multi-select bulk actions

All track list views support multi-select mode:

| View | Source |
|---|---|
| AlbumDetailView | `TrackRow` checkbox via existing selectable props |
| ArtistDetailView | Custom track rows with injected checkbox |
| PlaylistDetailView | Virtualized list; `VirtualizedTrackList` extended with `selectable`/`selected`/`onToggleSelect` |
| FavoritesView | Virtualized list via same props |
| DynamicSuggestView (Daily Q) | `TrackRow` checkbox |
| WeeklySuggestView | `TrackRow` checkbox |
| FavQView | `TrackRow` checkbox |
| TopQView | `TrackRow` checkbox |
| LabelView | Custom track rows with injected checkbox |

**BulkActionBar component** (`src/lib/components/BulkActionBar.svelte`):
- Sticky-bottom bar with count label, queue split-button (Add to Queue / Play Next), Add to Playlist, Add/Remove Favorites, Remove from Playlist
- Animated entry (`slideUp` 180 ms)
- `placement` prop retained for future use; all current views use bottom placement
- Positioned after all track rows; views already carry `padding-bottom: 100px` so no track is ever hidden

**Backend V2 commands added** (`src-tauri/src/commands_v2.rs`):
- `v2_add_tracks_to_queue_next` — insert tracks at front of queue
- `v2_bulk_toggle_favorites` — toggle favorite state for a list of track IDs in one call

**i18n keys added** (all four locales: en, es, de, fr):
- `actions.select`
- `actions.cancelSelection`
- `actions.selectedTracks` (interpolated: `{count} selected`)

### Custom artist images

- `customArtistImageStore` — Svelte store keyed by artist ID; populated from a bulk backend fetch on login
- Right-click context menu on any artist image opens a file picker; selected image is resized (max 800 px) and thumbnailed (200 px) in the Rust backend and stored in the app data directory
- V2 backend commands: `v2_set_custom_artist_image`, `v2_remove_custom_artist_image`, `v2_get_custom_artist_image`, `v2_bulk_get_custom_artist_images`
- `resolveArtistImage(id, fallback)` helper used across: `HomeView`, `SearchView`, `FavoritesView`, `ArtistDetailView`, `LabelView`
- Images sync immediately on assignment without requiring navigation

### Image lightbox

- `ImageLightbox` component: full-screen overlay, click-outside or Escape to dismiss, natural image dimensions
- Wired to artist photo (ArtistDetailView) and album artwork (AlbumDetailView)
- Round mask removed by design — images display at their natural crop

### Qobuz Radio

- Album radio button in AlbumDetailView header
- Artist radio and track radio in ArtistDetailView and track context menus
- Submenu lists available Qobuz radio variants for the selected artist/track
- V2 backend: `v2_create_qobuz_artist_radio`, `v2_create_qobuz_track_radio`

### Session restore

- App serializes active view + selected item IDs to a session file on navigation
- On launch, `restoreView` in `navigationStore` rehydrates without triggering data re-fetches
- Startup page setting in Settings → General: `home`, `library`, `purchases`, `last`

### Shareable and deep links — Odesli/Songlink integration

- `LinkResolverService` — backend service that accepts any streaming platform URL
- Phase 1: direct platform API calls for Spotify, Apple Music, Tidal, Deezer, YouTube Music, Amazon Music
- Phase 2: falls back to Odesli universal API (`song.link`) when direct resolution fails
- Falls back again to smart Qobuz search (title + artist) if Odesli has no Qobuz entry
- Share outward: track context menu → "Share via Songlink" copies a `song.link` URL to clipboard
- Paste inward: paste any recognized URL to open the Qobuz equivalent in-app

### Font selector

- `fontFamily` setting in Appearance; persisted in user config
- Applied via CSS custom property on `:root`; immediate effect
- Options include system-ui, Inter, Roboto, and a handful of monospace/serif choices

### System / Auto-theme

- `autoThemeStore` and V2 commands `v2_generate_auto_theme`, `v2_detect_system_theme`
- Desktop environment detection: KDE (reads `~/.config/kdeglobals`), GNOME (GSettings schema), others
- On DEs without an accent API: extracts a dominant palette from the current album artwork using a k-means color quantizer in the Rust backend
- Generated colors injected as CSS custom properties; can be individually overridden with the inline color picker in Appearance settings
- Bootstrap restore: theme is re-applied from stored config on startup before the first paint

---

## Redesigns

### Label view

Previous design was a flat list of popular tracks. New design is a full-page layout with:

1. **Hero header** — label logo/image, name, album count, carousel jump-links
2. **Popular Tracks section** — play-all controls, multi-select, show-more/show-less (5 → 20 → 50)
3. **Releases** — horizontal scroll carousel with "See All" pagination
4. **Playlists** — horizontal scroll carousel
5. **Artists** — horizontal scroll carousel with custom artist image support

---

## Improvements

### Audio

- **PipeWire force bit-perfect** — new option in Audio settings; sets `node.rate` to track's native sample rate and disables PipeWire's built-in resampler, delivering raw PCM to the hardware node without any mixer involvement
- Bit-perfect remains non-negotiable: `try_from_device_config()` path is preserved; this option is additive

### Performance

- **L1 cache: 400 MB, L2 cache: 800 MB** — raised from previous defaults; reduces backend re-fetches for users with large streaming histories
- Label view artist image carousel deferred to idle frames via batched `Promise.allSettled`; prevents paint jank on first render

### UI/UX

- **UI scale options** — 85% and 95% added between the existing 80% and 100% steps
- **Track multi-select persists during scroll** — selection state held in a `Set<number>` (not DOM state); survives virtual scroll culling in Favorites and Playlist views

---

## Bug Fixes

### Multi-select action bar positioning (ADR-001 + layout)

**Problem:** `BulkActionBar` placed before the column header (`position: sticky; bottom: 0`) was rendered at the *top* of the track list, not the bottom. On scroll, it floated over tracks.

**Root cause (ADR-001):** AlbumDetailView and WeeklySuggestView used `filter(t => ...)` inside async functions — `t` shadows the svelte-i18n `$t` store, breaking `vite-plugin-svelte`'s CSS extraction pipeline. This caused the entire component's CSS to be dropped at runtime.

**Fix:**
- `filter(t => ...)` → `filter(track => ...)` / `filter(trk => ...)` in all affected functions
- `BulkActionBar` moved to the end of the track row container, after `{/each}`, still inside the scrollable section
- All affected views already had `padding-bottom: 100px`; no track is ever hidden under the sticky bar

### Scroll position bleeding between items

**Problem:** `scrollPositions: Map<ViewType, number>` used a single key per view type. Opening Album B after Album A would restore Album A's saved scroll offset in Album B.

**Fix:** Key changed to `"viewType:itemId"` — e.g. `"album:MAQJXZA123456"`, `"artist:300213"`, `"playlist:42"`. Views without a specific item (search, favorites tabs) retain the view-type-only key unchanged. The `scrollKey()` helper builds the composite string; `saveScrollPosition` and `getSavedScrollPosition` accept an optional `itemId` parameter.

### Scroll position TTL

**Problem:** Saved positions were permanent; an album visited weeks ago would restore a stale offset.

**Fix:** `ScrollEntry` now stores `{ scrollTop, savedAt: Date.now() }`. `getSavedScrollPosition` returns 0 if `Date.now() - savedAt > 3_600_000` (1 hour). Constant `SCROLL_TTL_MS` at the top of `navigationStore.ts` for easy adjustment.

---

## Architecture (Internal)

> This section is for contributors. Users can skip it.

### V2 multi-crate migration (Codex)

The entire Tauri command layer was migrated from a monolithic `src-tauri/src/` module to a workspace of independent Rust crates with no Tauri dependency:

| Crate | Responsibility |
|---|---|
| `qbz-core` | Shared types, error types, DB utilities |
| `qbz-models` | Domain model structs (Track, Album, Artist, …) |
| `qbz-player` | Audio engine, queue, playback state |
| `qbz-audio` | PipeWire/ALSA backend wrappers |
| `qbz-qobuz` | Qobuz API client |
| `qbz-cache` | Two-level cache (L1 in-memory, L2 SQLite) |
| `qbz-library` | Local library scan and indexing |
| `qbz-integrations` | Last.fm, MPRIS, Odesli, Plex |

All frontend `invoke()` calls use `v2_*` or `runtime_*` prefixes. Legacy command modules (the old `src-tauri/src/library/commands.rs` et al.) have been deleted or reduced to stubs; no V2 command delegates to a legacy function. This separation means the player, library, and integrations layers can run headless (TUI, CLI) without Tauri being present.

The `commands_v2.rs` file added in this release adds the multi-select bulk commands and the purchase registry commands on top of the already-migrated stack.
