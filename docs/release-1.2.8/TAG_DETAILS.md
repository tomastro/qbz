# 1.2.8 — Mixtapes & Collections

A new way to curate your listening that lives entirely inside QBZ — no cloud
round-trips, no sync dance, and crucially no "tracks from Qobuz only"
restriction. Two first-class concepts land alongside the existing Qobuz
playlists: **Mixtapes** and **Collections**. Both are independent from
Qobuz's own playlist system, and once you've used them for a week you'll
notice they fill different gaps.

---

## Mixtape vs. Collection vs. Playlist

Three container types now coexist. They look similar on the surface but
each one serves a different need. Picking the right one is the difference
between building a catalogue and building a listening session.

### Playlist (the Qobuz one — unchanged)

A **cloud-native track list, owned by your Qobuz account**.

- The atomic unit is a **single track**.
- Lives on Qobuz's servers. Installs of QBZ on other machines see the
  same playlists because they're tied to your account.
- Everything in it has to exist in the **Qobuz catalogue** — no room for
  a local FLAC or a Plex track.
- Order is loose; shuffle is a first-class operation.
- Mental model: *"My favourites from the 80s"*, *"Discoveries this month"*.

### Mixtape (new)

A **private, cross-source, ordered listening session**.

- The atomic unit is **any object**: a whole album, a single track, or
  even a Qobuz playlist nested inside.
- Lives **only in your QBZ install** — nothing leaves the machine, nothing
  syncs to Qobuz.
- **Cross-source by design**: a Qobuz album can sit right next to a FLAC
  from your drive, followed by an album from your Plex library. No other
  container in the app can do this.
- **Order matters**. A Mixtape is meant to be played from start to finish
  — the cassette metaphor. Small and curated (five, ten items), not a
  catalogue.
- Mental model: *"Music for the drive to Valle"*, *"What I'm going to
  show my cousin"*, *"Saturday training tape"*.

### Collection (new)

A **large themed album shelf**.

- The atomic unit is **a whole album only** — no loose tracks, no nested
  playlists.
- Grows as large as you want: 20, 50, 100 albums.
- Order is softer than a Mixtape. Reproduction supports **album-shuffle**
  (you get a random album from the shelf, but its tracks still play in
  order from the top — not shuffled within themselves).
- Mental model: *"Everything Macross"*, *"2000s progressive metal"*,
  *"Radiohead's full discography"* — the last one has its own automated
  flavour we call an **Artist Collection**, built in one click by the
  new **Discography Builder**.

### In one line

- **Playlist** = a Qobuz-hosted track list.
- **Collection** = a private themed album shelf.
- **Mixtape** = a private, ordered, cross-source listening session.

The **Add to Mixtape/Collection** picker knows the difference: when you
add an album you can target either; when you add a track or a playlist,
only Mixtapes show up (a "shelf of albums" with a loose track inside
wouldn't make sense).

---

## Where to find them

- New entries in the sidebar under **My QBZ** (collapsible).
- **Add to Mixtape/Collection** is wired into every context menu where a
  track, album, or playlist can be actioned: Local Library, Artist page,
  Search results, Label view, Playlist detail, Album detail, the queue's
  item rows.
- Multi-select works in Local Library albums, in Mixtape/Collection
  detail rows, and inside the Discography Builder — paired with the
  bulk action bar so you can queue, add to playlist, or add to another
  Mixtape/Collection in one pass.

---

## Building an Artist Collection the easy way

For the common case of *"give me every Radiohead album in one shelf"*,
there's a new **Discography Builder** on every artist page. It fans out
across Qobuz **and** your local library **and** your Plex cache, groups
identical titles, lets you override the release type (Album / EP /
Single / Live / Compilation) when the auto-detection gets it wrong, and
stamps the result as an Artist Collection you can play or shuffle in
one click.

---

## Under the hood (for upgraders)

- New tables: `mixtape_collections`, `mixtape_collection_items` — with
  CASCADE deletion and additive migrations.
- New Tauri command surface: `v2_*_mixtape_*` for CRUD, `v2_enqueue_collection`
  for playback, `v2_skip_to_next_item` / `previous_item` for hop-by-album
  navigation inside a Mixtape, `v2_mixtape_item_exists` for duplicate
  pre-checks, `v2_mixtape_upload_custom_cover` / `remove_custom_cover`
  for user artwork.
- Duplicate handling is explicit: if you try to add an item that's
  already in the target, you get a confirmation dialog with **Add
  anyway** / **Skip duplicates** / **Cancel** — same pattern as the
  existing Qobuz-playlist duplicate check.
- Per-user cosmetic override for release-type labels (Album / EP /
  Single / Live / Compilation) in a local sidecar, never written back
  to Qobuz or the library DB.
- The Plex branch of the queue resolver routes `plex:` album keys to
  the Plex cache instead of `local_tracks`, so a Collection that starts
  with a Plex album plays correctly end-to-end.

---

Full changelog: (link added at tag time)
