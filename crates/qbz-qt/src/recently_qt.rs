//! Recently-played store — Slint-free port of `crates/qbz/src/recently.rs`.
//!
//! Backs the two Discover rails the port was missing: "Recently Played
//! Albums" (album carousel) and "Recently Played Tracks" (slim carousel), on
//! Home AND For You.
//!
//! It is the SAME file the Slint and Tauri builds use —
//! `<data_dir>/qbz/recently_played.json`, an object
//! `{ "tracks": [...], "albums": [...] }` with the pre-#567 bare-array form
//! migrated on read by deriving the album list from the track window. Nothing
//! here is per-user: the path is app-wide, exactly as in the reference, so a
//! history recorded by the Slint build shows up here and vice versa.
//!
//! Read is free (one small JSON file, no network, no session). [`record`] is
//! the write side; its call site is the playback track-start edge, which lives
//! in `playback_qt.rs` — see the GLUE note on the function.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How many recent tracks to keep (recently.rs `MAX_RECENT`).
const MAX_RECENT: usize = 24;
/// Independent album cap (#567) so a run of long albums cannot shrink the
/// distinct-album history.
const MAX_RECENT_ALBUMS: usize = 24;

/// One recently-played track. Every field defaults so a store written by an
/// older build (or a newer one with more fields) stays readable.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecentTrack {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub artwork_url: String,
    #[serde(default)]
    pub album_id: String,
    #[serde(default)]
    pub album_title: String,
    #[serde(default)]
    pub album_artist: String,
    #[serde(default)]
    pub album_artwork_url: String,
    #[serde(default)]
    pub quality_tier: String,
    #[serde(default)]
    pub quality_label: String,
    #[serde(default)]
    pub genre: String,
    /// Raw ISO album release date; localized at render time.
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub artist_id: Option<u64>,
    /// "qobuz" | "plex" | "local"; empty = legacy entry, treated as Qobuz.
    #[serde(default)]
    pub source: String,
}

/// One history entry by track id. The store is a short JSON list (24 rows
/// reach the rail), so a linear scan is cheaper than any index — and this is
/// the ONLY place that knows a non-Qobuz row's album key, which is what the
/// Plex cache is queried by.
pub fn find_track(id: &str) -> Option<RecentTrack> {
    load_tracks().into_iter().find(|t| t.id == id)
}

/// One recently-played album (its own history since #567).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecentAlbum {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub artist_id: Option<u64>,
    #[serde(default)]
    pub artwork_url: String,
    #[serde(default)]
    pub quality_tier: String,
    #[serde(default)]
    pub quality_label: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentStore {
    #[serde(default)]
    tracks: Vec<RecentTrack>,
    #[serde(default)]
    albums: Vec<RecentAlbum>,
}

fn store_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("recently_played.json"))
}

/// Pre-#567 derive: first-occurrence de-dup over the track window. Kept ONLY
/// for the legacy migration path in [`read_store`].
fn derive_albums(tracks: &[RecentTrack]) -> Vec<RecentAlbum> {
    let mut albums: Vec<RecentAlbum> = Vec::new();
    for track in tracks {
        if track.album_id.is_empty() || albums.iter().any(|a| a.id == track.album_id) {
            continue;
        }
        albums.push(RecentAlbum {
            id: track.album_id.clone(),
            title: track.album_title.clone(),
            artist: track.album_artist.clone(),
            artist_id: track.artist_id,
            artwork_url: track.album_artwork_url.clone(),
            quality_tier: track.quality_tier.clone(),
            quality_label: track.quality_label.clone(),
            genre: track.genre.clone(),
            release_date: track.release_date.clone(),
            source: track.source.clone(),
        });
    }
    albums
}

/// True when a stored track id names an EPHEMERAL (in-memory, session-only)
/// track and therefore must never be persisted here.
///
/// Ephemeral ids are synthetic and `>= EPHEMERAL_ID_FLOOR` (2^48). Qobuz ids
/// and `local_tracks` row ids parse far below the floor; Plex rating_keys and
/// the empty id do not parse as `i64` at all — all of those keep recording.
fn is_ephemeral_track_id(id: &str) -> bool {
    id.parse::<i64>()
        .is_ok_and(crate::local_ephemeral::is_ephemeral_id)
}

/// Read the whole store. Missing / unreadable -> empty (the rails then render
/// their placeholders, never an error).
fn read_store() -> RecentStore {
    let Some(path) = store_path() else {
        return RecentStore::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return RecentStore::default();
    };
    if let Ok(store) = serde_json::from_slice::<RecentStore>(&bytes) {
        return store;
    }
    let tracks: Vec<RecentTrack> = serde_json::from_slice(&bytes).unwrap_or_default();
    let albums = derive_albums(&tracks);
    RecentStore { tracks, albums }
}

fn write_store(store: &RecentStore) {
    let Some(path) = store_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("[qbz-qt] recently-played store dir failed: {e}");
            return;
        }
    }
    match serde_json::to_vec_pretty(store) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                log::warn!("[qbz-qt] recently-played write failed: {e}");
            }
        }
        Err(e) => log::warn!("[qbz-qt] recently-played serialize failed: {e}"),
    }
}

/// Recently-played tracks, newest first.
pub(crate) fn load_tracks() -> Vec<RecentTrack> {
    read_store().tracks
}

/// Recently-played albums, newest first.
pub(crate) fn load_albums() -> Vec<RecentAlbum> {
    read_store().albums
}

/// Namespaced cache ids for recently played Jellyfin tracks, newest first.
/// The hydration worker resolves them back to opaque server ids through the
/// cache, so history never needs to persist a credential-bearing URL.
pub(crate) fn jellyfin_recent_track_ids() -> Vec<i64> {
    read_store()
        .tracks
        .into_iter()
        .filter(|track| track.source == "jellyfin")
        .filter_map(|track| track.id.parse::<i64>().ok())
        .collect()
}

/// Drop every entry whose `album_id` is in `album_ids`; returns how many
/// TRACK entries went.
///
/// Called when a Local Library folder is removed, so its albums stop
/// lingering in Recently Played as rows that open onto nothing. Without it
/// the history keeps pointing at tracks whose database rows are gone —
/// visible as dead cards, not as an error.
///
/// Port of `crates/qbz/src/recently.rs:236-250` with ONE structural
/// difference: this store derives its album list from the track window
/// (`derive_albums`), so pruning the tracks is enough and the reference's
/// second `store.albums.retain` has no counterpart here. The `albums` field
/// still exists for the legacy migration path, so it is pruned too rather
/// than left holding stale ids.
pub(crate) fn prune_albums(album_ids: &[String]) -> usize {
    if album_ids.is_empty() {
        return 0;
    }
    let mut store = read_store();
    let tracks_before = store.tracks.len();
    let albums_before = store.albums.len();
    store
        .tracks
        .retain(|t| !album_ids.iter().any(|k| k == &t.album_id));
    store
        .albums
        .retain(|a| !album_ids.iter().any(|k| k == &a.id));
    let removed = tracks_before - store.tracks.len();
    if removed > 0 || albums_before != store.albums.len() {
        write_store(&store);
    }
    removed
}

/// Record a played track at the front of the track history (dedup by track
/// id, capped at [`MAX_RECENT`]) and its album at the front of the album
/// history (dedup by album id, capped at [`MAX_RECENT_ALBUMS`]).
///
/// The same write, told what the user was PLAYING FROM.
///
/// The context cannot ride on `RecentTrack` itself, and this is the constraint
/// that shapes the whole feature: `recently_played.json` is a DESTRUCTIVE round
/// trip between the two frontends. The Slint build deserializes it into its own
/// structs, serde silently drops fields it does not know, and its next write
/// puts the truncated object back — so a `context_kind` added to `RecentTrack`
/// would survive exactly until the other build played one track, intermittently
/// and with nothing logged. The gate therefore lives at the WRITE, and the file
/// on disk keeps the shape both builds agree on.
pub(crate) fn record_in_context(track: RecentTrack, context_kind: &str) {
    if track.id.is_empty() {
        return;
    }
    // EPHEMERAL — store invariant, not just a call-site courtesy. An ephemeral
    // id must never reach this JSON, whoever calls. Real ids parse well below
    // `EPHEMERAL_ID_FLOOR` (2^48) and Plex rating_keys do not parse as i64 at
    // all, so this rejects exactly the synthetic ids and nothing else. See the
    // long note on `record_queue_track` for the why.
    if is_ephemeral_track_id(&track.id) {
        return;
    }
    let mut store = read_store();
    // The ALBUM half is written only when the play was ABOUT an album.
    //
    // THE BUG THIS CLOSES: every track start wrote its album into
    // `store.albums`, whatever the user had actually chosen to play. A playlist
    // of 40 tracks from 40 different albums therefore pushed 40 entries into a
    // list capped at 24, and "Recently Played" became a list of that playlist's
    // contents — the playlist itself recorded nowhere. `""` (nothing stamped)
    // still counts: most of the untagged entry points ARE album listening, and
    // treating them as playlists would empty the album rail instead.
    let album_context = context_kind.is_empty() || context_kind == "album";
    if album_context && !track.album_id.is_empty() {
        store.albums.retain(|a| a.id != track.album_id);
        store.albums.insert(
            0,
            RecentAlbum {
                id: track.album_id.clone(),
                title: track.album_title.clone(),
                artist: track.album_artist.clone(),
                artist_id: track.artist_id,
                artwork_url: track.album_artwork_url.clone(),
                quality_tier: track.quality_tier.clone(),
                quality_label: track.quality_label.clone(),
                genre: track.genre.clone(),
                release_date: track.release_date.clone(),
                source: track.source.clone(),
            },
        );
        store.albums.truncate(MAX_RECENT_ALBUMS);
    }
    store.tracks.retain(|t| t.id != track.id);
    store.tracks.insert(0, track);
    store.tracks.truncate(MAX_RECENT);
    write_store(&store);
}

/// Quality captured by both history stores. QueueTrack's DSD rate is already
/// in kHz, so it must reach the DSD classifier before generic PCM formatting.
fn recorded_quality(bit_depth: Option<u32>, sample_rate: Option<f64>) -> (&'static str, String) {
    if bit_depth == Some(1) {
        return ("dsd", crate::quality_qt::dsd_multiple_label(sample_rate));
    }
    let tier = crate::home_qt::quality_tier_from_depth(bit_depth);
    let detail = crate::home_qt::quality_detail_from_parts(bit_depth, sample_rate);
    let label = if tier.is_empty() {
        String::new()
    } else {
        format!(
            "{}: {detail}",
            if tier == "hires" { "Hi-Res" } else { "CD" }
        )
    };
    (tier, label)
}

/// Correct old one-bit snapshots at display time, without rewriting history.
/// The former PCM formatter divided a kHz DSD rate a second time, producing
/// e.g. `CD: 1-bit / 2.8224 kHz`; builds fed Hz stored `2822.4 kHz` instead.
pub(crate) fn display_quality(tier: String, label: String) -> (String, String) {
    let detail = label.trim().strip_prefix("CD: ").unwrap_or(label.trim());
    let Some(rate_text) = detail.strip_prefix("1-bit / ") else {
        return (tier, label);
    };
    let rate = rate_text
        .strip_suffix(" kHz")
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map(|rate| if rate < 1000.0 { rate * 1000.0 } else { rate });
    ("dsd".into(), crate::quality_qt::dsd_multiple_label(rate))
}

/// Record the current queue track into BOTH local play stores: the
/// recently-played history (this module) and the album play-count history
/// (`qbz_app::settings::album_play_history`, which ranks the "Most Played
/// Albums" rail). Keeps the playback glue a single line.
///
/// The queue model carries no album genre / release date (Slint stamps those
/// from a side `ALBUM_META` map filled by the album-fetch path), so a card
/// recorded here shows title / artist / artwork / quality and no genre-date
/// overlay line — never a wrong one.
///
/// WIRED: called from the DE-DUPED playback track-change edge in
/// `playback_qt::start_poll_loop`, next to
/// `integrations_qt::on_track_change_edge`. Both this rail and the "Most
/// Played Albums" rail therefore contain LOCAL and PLEX rows (the album play
/// history stores `source` verbatim and never filters on it), which is why
/// every album play/enqueue entry point has to route a group key to the local
/// path instead of `get_album` — see `playback_qt::is_local_album`.
pub(crate) fn record_queue_track(track: &qbz_models::QueueTrack) {
    // EPHEMERAL — owner rule: "ephemeral solo se reproduce y ya […] honramos el
    // nombre ephemeral: no deja rastros en las bibliotecas, ni now playing ni
    // nada." An ephemeral folder lives in memory for one session and may be
    // added to the QUEUE and to nothing else. Recording it here would write it
    // into TWO app-wide persistent stores that outlive the process —
    // `recently_played.json` (Recently Played Albums / Tracks) and
    // `album_play_history.db` (Most Played Albums) — naming a folder that is
    // not in the library, under synthetic ids that are meaningless after
    // restart. Neither store can express "temporary", so the only correct move
    // is to never write. Guarded HERE, above both writes, so any future caller
    // inherits the rule; the poll-loop call site skips it earlier too, purely
    // to save the queue-state fetch.
    //
    // NOTE: the Slint reference (`qbz/src/playback.rs::record_recent`) has no
    // such guard — this is the rule being implemented for the first time, not
    // a Qt-only regression being patched.
    if crate::local_ephemeral::is_ephemeral_id(track.id as i64) {
        log::debug!(
            "[qbz-qt] ephemeral track {} not recorded in play history (no library traces)",
            track.id
        );
        return;
    }
    if track.source.as_deref() == Some("jellyfin") {
        if let Some(item_id) = track.source_item_id_hint.as_ref() {
            crate::media_sync_qt::prioritize_jellyfin_quality(vec![item_id.clone()], true);
        }
    }
    let (tier, quality_label) = recorded_quality(track.bit_depth, track.sample_rate);
    let artwork = track.artwork_url.clone().unwrap_or_default();
    let album_id = track.album_id.clone().unwrap_or_default();
    let source = track.source.clone().unwrap_or_default();
    let artist_id = track.artist_id.map(|id| id.to_string()).unwrap_or_default();

    // What the user chose to play, as opposed to where the audio comes from
    // (`source`). Two different axes with confusingly similar names: a LOCAL
    // playlist is `context_kind = "playlist"` AND `source = "local"`.
    let context_kind = track.context_kind.clone().unwrap_or_default();
    let context_id = track.context_id.clone().unwrap_or_default();

    // A playlist play is recorded AS a playlist play, in its own store, and
    // does not touch either album history. The meta (title, owner, cover) was
    // already upserted by the view that started playback — the edge only ever
    // sees a QueueTrack, and resolving an id back into a header on every track
    // change would be a fetch per track.
    if context_kind == "playlist" {
        qbz_app::settings::playlist_play_history::record_playlist_play(&context_id);
    }

    qbz_app::settings::album_play_history::record_album_play(
        qbz_app::settings::album_play_history::AlbumPlayMeta {
            album_id: &album_id,
            title: &track.album,
            artist: &track.artist,
            artist_id: &artist_id,
            artwork_url: &artwork,
            quality_tier: tier,
            quality_label: &quality_label,
            year: "",
            source: &source,
            context_kind: &context_kind,
        },
    );

    record_in_context(
        RecentTrack {
            id: track.id.to_string(),
            title: track.title.clone(),
            subtitle: track.artist.clone(),
            artwork_url: artwork.clone(),
            album_id,
            album_title: track.album.clone(),
            album_artist: track.artist.clone(),
            album_artwork_url: artwork,
            quality_tier: tier.to_string(),
            quality_label,
            genre: String::new(),
            release_date: String::new(),
            artist_id: track.artist_id,
            source,
        },
        &context_kind,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_history_keeps_artist_identity_and_reads_old_snapshots() {
        let tracks = vec![RecentTrack {
            album_id: "catalog-album".into(),
            album_artist: "Led Zeppelin".into(),
            artist_id: Some(123),
            source: "qobuz".into(),
            ..Default::default()
        }];
        let albums = derive_albums(&tracks);
        let encoded = serde_json::to_string(&albums[0]).unwrap();
        let album: RecentAlbum = serde_json::from_str(&encoded).unwrap();
        let card = crate::home_qt::map_recent_album(album);
        assert_eq!(card.artist_id, "123");
        assert!(card.history_artist_link);
        let old: RecentAlbum =
            serde_json::from_str(r#"{"id":"old-album","artist":"Led Zeppelin"}"#).unwrap();
        assert_eq!(old.artist_id, None);
        let card = crate::home_qt::map_recent_album(old);
        assert!(card.artist_id.is_empty());
        assert!(
            card.history_artist_link,
            "old albums still need a clickable name"
        );
    }

    #[test]
    fn local_dsd_queue_metadata_records_the_multiple_in_both_histories() {
        for (rate, expected) in [
            (2_822_400.0, "DSD64"),
            (5_644_800.0, "DSD128"),
            (11_289_600.0, "DSD256"),
            (22_579_200.0, "DSD512"),
        ] {
            let native = qbz_library::LocalTrack {
                format: qbz_library::AudioFormat::Dsd,
                bit_depth: Some(1),
                sample_rate: rate,
                ..Default::default()
            };
            let queued = crate::local_playback::local_queue_track(&native);
            assert_eq!(
                recorded_quality(queued.bit_depth, queued.sample_rate),
                ("dsd", expected.into())
            );
            assert_eq!(
                recorded_quality(Some(1), Some(rate)),
                ("dsd", expected.into())
            );
        }
        assert_eq!(
            recorded_quality(Some(16), Some(44.1)),
            ("cd", "CD: 16-bit / 44.1 kHz".into())
        );
        assert_eq!(
            recorded_quality(Some(24), Some(96_000.0)),
            ("hires", "Hi-Res: 24-bit / 96 kHz".into())
        );
    }

    #[test]
    fn legacy_one_bit_album_snapshots_render_as_dsd_without_a_replay() {
        for (label, expected) in [
            ("CD: 1-bit / 2.8224 kHz", "DSD64"),
            ("CD: 1-bit / 2822.4 kHz", "DSD64"),
            ("CD: 1-bit / 5.6448 kHz", "DSD128"),
            ("CD: 1-bit / 11.2896 kHz", "DSD256"),
            ("CD: 1-bit / 22.5792 kHz", "DSD512"),
        ] {
            let recent = crate::home_qt::map_recent_album(RecentAlbum {
                quality_tier: "cd".into(),
                quality_label: label.into(),
                ..Default::default()
            });
            let played = crate::home_qt::map_played_album(
                qbz_app::settings::album_play_history::AlbumPlayRow {
                    quality_tier: "cd".into(),
                    quality_label: label.into(),
                    ..Default::default()
                },
            );
            for card in [recent, played] {
                assert_eq!(card.quality_tier, "dsd");
                assert_eq!(card.quality_label, expected);
                assert_eq!(card.quality_detail, expected);
            }
        }
        for (tier, label) in [
            ("hires", "Hi-Res: 24-bit / 192 kHz"),
            ("cd", "CD: 16-bit / 44.1 kHz"),
            ("dsd", "DSD128"),
        ] {
            assert_eq!(
                display_quality(tier.into(), label.into()),
                (tier.into(), label.into())
            );
        }
    }

    #[test]
    fn legacy_bare_array_migrates_to_derived_albums() {
        let tracks = vec![
            RecentTrack {
                id: "1".into(),
                album_id: "a".into(),
                album_title: "A".into(),
                ..Default::default()
            },
            RecentTrack {
                id: "2".into(),
                album_id: "a".into(),
                ..Default::default()
            },
            RecentTrack {
                id: "3".into(),
                album_id: "b".into(),
                ..Default::default()
            },
        ];
        let derived = derive_albums(&tracks);
        assert_eq!(derived.len(), 2);
        assert_eq!(derived[0].id, "a");
        assert_eq!(derived[0].title, "A");
        assert_eq!(derived[1].id, "b");
    }

    /// The owner's ephemeral rule: a session-only folder leaves no trace in the
    /// play history. Pure predicate test — it never touches the real store.
    #[test]
    fn ephemeral_ids_are_rejected_by_the_history_store() {
        let floor = qbz_library::ephemeral::EPHEMERAL_ID_FLOOR;
        assert!(is_ephemeral_track_id(&floor.to_string()));
        assert!(is_ephemeral_track_id(&(floor + 42).to_string()));
    }

    #[test]
    fn real_ids_still_record() {
        // Qobuz track id, local_tracks row id, a Plex rating_key and the empty
        // id: none of them may be mistaken for ephemeral.
        assert!(!is_ephemeral_track_id("139578884"));
        assert!(!is_ephemeral_track_id("2954"));
        assert!(!is_ephemeral_track_id("plex-12345"));
        assert!(!is_ephemeral_track_id(""));
    }

    #[test]
    fn tracks_without_an_album_are_skipped_by_the_derive() {
        let tracks = vec![RecentTrack {
            id: "1".into(),
            ..Default::default()
        }];
        assert!(derive_albums(&tracks).is_empty());
    }
}
