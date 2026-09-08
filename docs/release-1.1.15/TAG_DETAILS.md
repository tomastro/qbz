# 1.1.15 — Your Mixes, Purchases & Multi-select

Largest feature release since 1.1.8. Ten new user-facing features, a completely rebuilt Label view, and targeted fixes for scroll behavior and layout.

## New Features

  - **Purchases** — browse and re-download your Qobuz purchase history directly in the app; local download registry persists across restarts. Experimental since it was blindfold developed haha. Opt-in in settings
  - **Your Mixes** — Daily Q, Weekly Q, Fav Q, and Top Q surfaced on the Home view, each with its own full view, artwork, queue controls, and search/filter
  - **Multi-select tracks** — checkbox selection mode across all track lists (Album, Artist, Playlist, Favorites, Label, and all four Mixes views); bulk queue, playlist, and favorites actions via a sticky action bar
  - **Custom artist images** — right-click any artist photo to replace it with a local image; persisted per artist and reflected immediately everywhere the artist appears
  - **Image lightbox** — click any album artwork or artist photo for a full-screen overlay; Escape or click-outside to dismiss
  - **Qobuz Radio** — start artist or track radio from any context menu or artist page; submenu lists available stations. QBZ radio is still available if you liked it.
  - **Session restore** — app returns to your last view on launch; configurable startup page in Settings → General (Home, Library, Purchases, or Last). Opt-in in settings
  - **Shareable links** — paste any Songlink/Odesli or streaming platform URL to open the Qobuz match in-app; share tracks outward via the track context menu. Ctrl+L to launch opener. 
  - **Font selector** — choose UI font family in Settings → Appearance; takes effect immediately
  - **System theme** — auto-generates a color scheme from your desktop accent color (KDE/GNOME) or current album artwork; generated swatches are editable with the inline color picker

## Improvements

  - **Label view redesigned** — completely rebuilt as a multi-section page: hero header, Popular Tracks with play-all and multi-select, Releases carousel, Playlists, and Artists with custom image support
  - **Cache limits raised** — L1 increased to 400 MB, L2 to 800 MB, reducing re-fetches on large libraries. This will be configurable soon. 
  - **UI scale options** — 85 % and 95 % steps added to the existing scale selector in Appearance
  - **PipeWire force bit-perfect** — new audio option to bypass the volume mixer and deliver raw PCM directly to the hardware sink; non-negotiable bit-perfect path preserved. Experimental, please use with care. 
  - **Custom artist image sync** — images assigned in Artist Detail reflect immediately in Home, Search, Favorites, and Label without restarting
  - **More viewers** — For full-screen playback sessions, there are now more options besides bars. 

## Bug Fixes

  - **Scroll position bleeding between items** — opening a second album no longer restores the first album's saved scroll offset; positions are now scoped per item ID
  - **Scroll position TTL** — saved positions expire after one hour; stale offsets from days-old visits no longer teleport you mid-list
  - **PostCSS CSS-extraction failures** — views that used `t` as an arrow-function parameter (shadowing the svelte-i18n store) had their CSS silently dropped; fixed across AlbumDetailView, WeeklySuggestView, and related helpers
  - **Auto reload audio settings** — In some DAC models, especially portable ones, the DAC name may change upon restarting. It now auto-refreshes when the app is launched. It is opt-in so as not to affect those who do not need it. 
  - **Settings that do not persist** — fixed several settings that were not connected and did not respect the user's selection.
  - **Play queue move and clear bugs** — track reordering now lands in the correct position; clearing the queue while a track is playing no longer skips the first enqueued track afterward (by @vorce, PR #79)

---

Special thanks to **@jrgn9**, **@ThaYke**, and **@turby666** for QA testing this release.
As always, thank you to everyone who reports bugs, tests edge cases, and takes the time to give feedback — you are the best QA team.

Full changelog: https://github.com/vicrodh/qbz/compare/v1.1.14...v1.1.15