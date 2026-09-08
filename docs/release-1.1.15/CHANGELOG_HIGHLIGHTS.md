# QBZ 1.1.15 Release Highlights

## New Features

- **Purchases** — browse your Qobuz purchase history, track local downloads, re-download any purchase
- **Your Mixes** — Daily Q, Weekly Q, Fav Q, and Top Q on the Home view; each with a full dedicated view
- **Multi-select tracks** — checkbox selection mode across all track lists; bulk queue, playlist, and favorites actions
- **Custom artist images** — right-click any artist photo to replace it; persisted per artist, synced globally across all views
- **Image lightbox** — click any album artwork or artist photo to view full-size
- **Qobuz Radio** — start artist or track radio from any view; submenu shows available stations
- **Session restore** — app returns to your last view on launch; configurable startup page in Settings → General
- **Shareable links** — paste any Songlink/Odesli URL to open the Qobuz equivalent; share tracks outward via the track context menu
- **Font selector** — choose UI font family in Settings → Appearance
- **System theme** — auto-generates a color scheme from your DE accent or current album art; swatches are editable

## Improvements

- **Label view** completely rebuilt: Popular Tracks, Releases, Playlists, and Artists sections
- **Cache raised** to L1 400 MB / L2 800 MB
- **UI scale** — 85% and 95% options added
- **PipeWire force bit-perfect** — bypass volume mixer for raw PCM delivery
- **Custom artist image sync** — changes in Artist Detail reflect immediately everywhere

## Bug Fixes

- Multi-select action bar no longer overlaps the last visible track
- Scroll position is now scoped per item (album, artist, playlist) — different items no longer share positions
- Saved scroll positions expire after 1 hour
- PostCSS CSS-extraction failures caused by `t`-as-parameter shadowing fixed in AlbumDetailView, WeeklySuggestView, and related views
