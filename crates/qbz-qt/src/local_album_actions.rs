//! Local album ACTIONS — everything the routed local album page
//! (`qml/views/LocalAlbumView.qml` + `qml/views/local/LocalAlbumHeader.qml`)
//! drives that is not a plain query: the VERSION picker, the per-disc "Disc N"
//! menu, the artist-NAME route into the Local Library Artists tab, the
//! per-row-artwork appearance mirror, and the album-level actions.
//!
//! Ported 1:1 from `album/LocalAlbumView.slint` (617 lines) plus its Rust glue
//! (`crates/qbz/src/local_library.rs`: `open_local_album`,
//! `apply_album_version`, `album_version_dir`, `current_album_version_tracks`,
//! `current_album_disc_tracks`; `crates/qbz/src/main.rs`: the
//! `LocalAlbumActions` handlers).
//!
//! VERSIONS: a "version" is a distinct PHYSICAL copy of the album — a distinct
//! source directory (`LocalTrack.album_group_key`). Metadata identity mode can
//! fold several directories into one album card, and merging their tracks
//! would render a duplicated track list; splitting by directory is what stops
//! that. The split is cached in `LocalState` so the picker switches with NO DB
//! round-trip (the Slint's `ALBUM_VERSIONS` static, 1:1).
//!
//! Metadata editing is version-scoped and opens an app-wide modal. The Rust
//! controller owns the physical paths and verifies direct writes before it
//! republishes the album. Playlist and MyQBZ actions likewise operate on the
//! selected version rather than the logical collection.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cxx_qt_lib::QString;
use qbz_app::settings::local_favorites::LocalFavItem;
use qbz_app::shell::AppRuntime;
use qbz_core::LoggingAdapter;
#[cfg(test)]
use qbz_library::AudioFormat;
use qbz_library::LocalTrack;
use qbz_models::QueueTrack;
use serde::Serialize;

use crate::local_bridge::ui;
use crate::local_playback::{fill_missing_covers, local_queue_track};
use crate::local_rows::{
    album_favorite_source, album_key, badge_source, badge_source_raw, map_track,
    media_quality_rank, tier_of, to_json, total_duration, AlbumMediaVariant, AlbumRow, TrackRow,
};
use crate::local_state::{state, with_art};

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

// ---------------------------------------------------------------------------
// The routed page's document (the QML contract)
// ---------------------------------------------------------------------------

/// One selectable physical copy — the Slint's `LocalAlbumVersion`
/// (album/LocalAlbumView.slint:42), consumed by
/// `qml/views/local/VersionPicker.qml`.
#[derive(Clone, Default, Serialize)]
pub struct AlbumVersion {
    /// Stable identity of the FILTERED physical row set. QML uses it to keep
    /// an unchanged selected version mounted when a newly enabled source only
    /// adds another picker option.
    pub key: String,
    /// "Remastered", "Deluxe Edition", … when metadata makes it inferable.
    pub version: String,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    /// "24-bit / 96 kHz · FLAC" (quality + container).
    pub quality: String,
    /// RAW `local_tracks.source` ("" | "qobuz_download" | "qobuz_purchase" |
    /// "plex") — `SourceIcon.qml` keys its glyph/tint off exactly these.
    pub source: String,
}

/// The album-detail HEADER: the shared `AlbumRow` plus the two fields only the
/// routed page uses. Flattened so QML keeps reading `album.title`,
/// `album.artKey`, … unchanged.
#[derive(Clone, Serialize)]
pub struct AlbumHeaderDoc {
    #[serde(flatten)]
    pub row: AlbumRow,
    pub versions: Vec<AlbumVersion>,
    #[serde(rename = "versionIndex")]
    pub version_index: i32,
}

/// One disc of a multi-disc album: what the divider needs and nothing else.
///
/// A SEPARATE ARRAY, not extra columns on `TrackRow`. The divider is one item
/// per disc — two to four of them — where the track list is the freeze surface
/// that can run to hundreds of rows; paying for a title and an art key on
/// every row to label three would be the wrong trade.
#[derive(Clone, Serialize)]
pub struct DiscRow {
    /// `TrackRow.disc` for the tracks this describes.
    pub disc: u32,
    /// The disc's OWN name ("Das Rheingold", "TV Series Soundtrack #01"), or
    /// empty when the box does not name its discs — QML then draws the bare
    /// "Disc N" it drew before.
    pub title: String,
    /// Art index key for this disc's own cover (the first track's), so the
    /// divider can show the disc's artwork rather than the box's.
    #[serde(rename = "artKey")]
    pub art_key: String,
    /// Absolute path to THIS disc's own cover, or empty.
    ///
    /// Resolved here rather than read off the track, because the scan-time
    /// `artwork_path` is deliberately biased to the ALBUM ROOT
    /// (`find_folder_artwork` gives the root a +5 bonus), so every disc of a
    /// box carries the SAME file and a per-disc thumbnail drawn from it would
    /// be N copies of one image.
    pub cover: String,
}

/// `{album:{…}, tracks:[…], discs:[…]}` — the `localAlbumJson` document.
#[derive(Clone, Serialize)]
pub struct AlbumDetailDoc {
    pub album: AlbumHeaderDoc,
    pub tracks: Vec<TrackRow>,
    /// EMPTY for a single-disc album. Present only so a multi-disc box can
    /// label and illustrate its dividers — see `DiscRow`.
    pub discs: Vec<DiscRow>,
    /// Cold-start fallback for the global compact-header appearance pref.
    /// LocalAlbumView switches to the live settings document once published.
    #[serde(rename = "compactHeader")]
    pub compact_header: bool,
    /// Cold-start fallback for the same artwork-header preference consumed
    /// by Qobuz AlbumView. The live settings document wins once available.
    #[serde(rename = "headerGradient")]
    pub header_gradient: bool,
}

/// The bounded payload consumed by one expanded row in the Genres browser.
/// It deliberately reuses `DiscRow`: AlbumView and Genres must not disagree
/// about a box set's per-disc subtitle or cover merely because they are two
/// surfaces over the same selected physical version.
#[derive(Clone, Serialize)]
pub struct GenreAlbumDetailDoc {
    /// Version-specific artwork key. Empty only when no physical copy has a
    /// cover, in which case QML retains the logical card placeholder.
    #[serde(rename = "artKey")]
    pub art_key: String,
    pub tracks: Vec<TrackRow>,
    pub discs: Vec<DiscRow>,
    pub versions: Vec<AlbumVersion>,
    #[serde(rename = "versionIndex")]
    pub version_index: i32,
}

// ---------------------------------------------------------------------------
// Version splitting (local_library.rs `open_local_album` 1:1)
// ---------------------------------------------------------------------------

/// Shared quality rank for ordering versions (highest-quality first).
fn version_rank(t: &LocalTrack) -> (u8, u32, u64) {
    media_quality_rank(&t.format, t.bit_depth, t.sample_rate)
}

/// Group the album's tracks by SOURCE DIRECTORY, sort each copy by
/// (disc, track) and put the best-quality copy first, so the default
/// selection is the highest-res one.
pub fn split_versions(tracks: Vec<LocalTrack>) -> Vec<(String, Vec<LocalTrack>)> {
    let mut groups: HashMap<String, Vec<LocalTrack>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for t in tracks {
        let key = t.album_group_key.clone();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(t);
    }
    let mut versions: Vec<(String, Vec<LocalTrack>)> = order
        .into_iter()
        .filter_map(|k| {
            groups.remove(&k).map(|mut v| {
                v.sort_by_key(|t| (t.disc_number.unwrap_or(1), t.track_number.unwrap_or(0)));
                (k, v)
            })
        })
        .collect();
    versions.sort_by(|a, b| {
        let qa = a.1.iter().map(version_rank).max().unwrap_or((0, 0, 0));
        let qb = b.1.iter().map(version_rank).max().unwrap_or((0, 0, 0));
        qb.cmp(&qa)
            .then_with(|| b.1.len().cmp(&a.1.len()))
            .then_with(|| a.0.cmp(&b.0))
    });
    versions
}

/// A version's picker entry (`version_label` + `version_source` 1:1).
fn version_info(tracks: &[LocalTrack]) -> AlbumVersion {
    match tracks.iter().max_by_key(|track| version_rank(track)) {
        Some(t) => {
            let mut identity = std::collections::hash_map::DefaultHasher::new();
            for track in tracks {
                track.album_group_key.hash(&mut identity);
                track.source.hash(&mut identity);
                track.id.hash(&mut identity);
                track.file_path.hash(&mut identity);
                track.disc_number.hash(&mut identity);
                track.track_number.hash(&mut identity);
            }
            let detail = crate::local_rows::detail_of(&t.format, t.bit_depth, t.sample_rate);
            let fmt = t.format.to_string();
            let raw_source = badge_source_raw(t.source.as_deref());
            AlbumVersion {
                key: format!("{:016x}", identity.finish()),
                version: crate::local_albums::edition_descriptor(&t.album_group_title),
                track_count: tracks.len() as u32,
                quality: if detail.is_empty() {
                    fmt
                } else {
                    format!("{detail} · {fmt}")
                },
                source: if raw_source.is_empty() {
                    badge_source(t.source.as_deref())
                } else {
                    raw_source
                },
            }
        }
        None => AlbumVersion::default(),
    }
}

/// The selected copy owns the cover when it has one. A coverless selected
/// copy falls back to the first artwork-bearing copy in `versions`, which is
/// already ordered by audio quality then track count. Returning the source
/// beside the token is essential: a Plex metadata key is meaningless when
/// resolved through Jellyfin credentials (and vice versa).
fn version_artwork_ref(
    selected: &[LocalTrack],
    versions: &[(String, Vec<LocalTrack>)],
) -> Option<(Option<String>, String)> {
    let find = |tracks: &[LocalTrack]| {
        tracks
            .iter()
            .find_map(|track| {
                track
                    .collection_artwork_path
                    .as_ref()
                    .filter(|path| !path.is_empty())
                    .map(|path| (track.source.clone(), path.clone()))
            })
            .or_else(|| {
                tracks.iter().find_map(|track| {
                    track
                        .artwork_path
                        .as_ref()
                        .filter(|path| !path.is_empty())
                        .map(|path| (track.source.clone(), path.clone()))
                })
            })
    };
    find(selected).or_else(|| {
        versions
            .iter()
            .find_map(|(_, tracks)| find(tracks.as_slice()))
    })
}

/// A cover request is keyed by physical VERSION, not only logical album.
/// Reusing `album:{id}` let an earlier async resolve publish after the picker
/// changed and overwrite the newly selected edition's art.
fn album_version_art_key(id: &str, index: usize) -> String {
    format!("album-version:{id}:{index}")
}

/// Open `id`: cache its versions, select the best-quality one and return its
/// document. Called by `local_albums::load_album_detail_blocking` (BLOCKING
/// context — no Qt, no await).
pub fn open_versions(id: &str, tracks: Vec<LocalTrack>) -> Option<AlbumDetailDoc> {
    let versions = split_versions(tracks);
    if versions.is_empty() {
        return None;
    }
    // Keep the RAW rows of EVERY version so a context-menu enqueue on a Plex
    // detail row resolves without a DB id (`local_playback::find_track_blocking`).
    let all: Vec<LocalTrack> = versions
        .iter()
        .flat_map(|(_, v)| v.iter().cloned())
        .collect();
    state(|s| {
        s.album_id = id.to_string();
        s.album_versions = versions;
        s.album_version_index = 0;
        s.detail_raw = all;
    });
    version_doc(id, 0)
}

/// Build the document for version `index` of the OPEN album. Reads the cached
/// split — no DB round-trip (the Slint's `apply_album_version`).
pub fn version_doc(id: &str, index: usize) -> Option<AlbumDetailDoc> {
    let (infos, tracks, artwork) = state(|s| {
        let infos: Vec<AlbumVersion> = s
            .album_versions
            .iter()
            .map(|(_, v)| version_info(v))
            .collect();
        let tracks = s
            .album_versions
            .get(index)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let artwork = version_artwork_ref(&tracks, &s.album_versions);
        (infos, tracks, artwork)
    });
    if tracks.is_empty() {
        return None;
    }
    let mut row = album_header(id, &tracks);
    row.art_key = album_version_art_key(id, index);
    // Register the album cover + the per-row sources in the art index (the
    // windowed artwork channel resolves them; nothing rides the document).
    let rows = with_art(|art| {
        if let Some((source, path)) = artwork.as_ref() {
            if let Some(token) = crate::local_rows::art_token(source.as_deref(), path) {
                art.insert(row.art_key.clone(), token);
            }
        } else {
            art.remove(&row.art_key);
        }
        tracks
            .iter()
            .map(|t| map_track(t, art))
            .collect::<Vec<TrackRow>>()
    });
    let discs = disc_rows(&tracks, &row.title);
    Some(AlbumDetailDoc {
        album: AlbumHeaderDoc {
            row,
            versions: infos,
            version_index: index as i32,
        },
        tracks: rows,
        discs,
        compact_header: crate::settings_qt::pref_bool("compact_album_header", false),
        header_gradient: crate::settings_qt::pref_bool("album_header_gradient", true),
    })
}

/// One row per disc of a MULTI-disc album, for the track list's divider.
///
/// WHY THIS IS NEEDED AT ALL. In FOLDER grouping a box set is deliberately ONE
/// album, so `TrackRow.album` is `album_group_title` — the group name with the
/// disc suffix STRIPPED (metadata.rs::strip_disc_suffix). "Box (Disc 1)" and
/// "Box (Disc 2)" both collapse to "Box", which is exactly what makes the box
/// hold together, and it is also why the divider had nothing to say: the only
/// thing distinguishing disc 1 from disc 2 in the published document was the
/// integer. Owner report 2026-08-22, a Saint Seiya box whose discs each have
/// their own name and cover: "el separador no hizo nada al respecto".
///
/// METADATA grouping shows them correctly for an unrelated reason — there each
/// disc is its OWN album row, keyed `album || artist`, so it keeps its raw tag
/// and its own artwork. Nothing below changes that path.
///
/// The title is resolved in this order, most trustworthy first:
///   1. the disc FOLDER's own titled tail ("Disc 1 - Rheingold" -> "Rheingold").
///      This is where a box that names its discs usually says so, and it is the
///      only source the tag cannot contradict.
///   2. the track's RAW album tag, when it names something the group title does
///      not already say. Compared AFTER stripping the disc suffix, so a tag of
///      "Box (Disc 2)" — which carries no name, only a number — is correctly
///      rejected instead of being echoed next to the "Disc 2" label.
///   3. nothing, and the divider stays the bare "Disc N" it has always been.
///
/// EMPTY for a single-disc album: there is no divider there to feed.
pub(crate) fn disc_rows(tracks: &[LocalTrack], group_title: &str) -> Vec<DiscRow> {
    use std::collections::BTreeMap;
    let mut first: BTreeMap<u32, &LocalTrack> = BTreeMap::new();
    for t in tracks {
        first.entry(t.disc_number.unwrap_or(1)).or_insert(t);
    }
    if first.len() < 2 {
        return Vec::new();
    }
    first
        .into_iter()
        .map(|(disc, t)| DiscRow {
            disc,
            title: crate::local_rows::disc_display_title(t, group_title),
            art_key: crate::local_rows::track_key(t.id),
            cover: crate::local_rows::disc_cover_url(t),
        })
        .collect()
}

/// Map one selected physical version for Genres Details while carrying the
/// compact picker metadata for every other version. The heavy track rows stay
/// bounded to the selected copy; switching copies republishes this document
/// from `LocalState::genre_detail_versions` without another database query.
pub(crate) fn genre_detail_doc(
    album_id: &str,
    versions: &[(String, Vec<LocalTrack>)],
    version_index: usize,
) -> GenreAlbumDetailDoc {
    let tracks = versions
        .get(version_index)
        .map(|(_, tracks)| tracks.as_slice())
        .unwrap_or_default();
    let group_title = tracks
        .first()
        .map(|track| track.album_group_title.as_str())
        .unwrap_or_default();
    let discs = disc_rows(tracks, group_title);
    let artwork = version_artwork_ref(tracks, versions);
    let art_key = if artwork.is_some() {
        album_version_art_key(album_id, version_index)
    } else {
        String::new()
    };
    let rows = with_art(|art| {
        if let Some((source, path)) = artwork.as_ref() {
            if let Some(token) = crate::local_rows::art_token(source.as_deref(), path) {
                art.insert(art_key.clone(), token);
            }
        }
        tracks
            .iter()
            .map(|track| map_track(track, art))
            .collect::<Vec<_>>()
    });
    GenreAlbumDetailDoc {
        art_key,
        tracks: rows,
        discs,
        versions: versions
            .iter()
            .map(|(_, tracks)| version_info(tracks))
            .collect(),
        version_index: version_index as i32,
    }
}

/// Resolve one Genres album's selected physical copy without mutating the
/// cache. The caller publishes both values only after its generation check;
/// changing the cache here would let a superseded async request win.
pub(crate) fn genre_version_selection(
    album_id: &str,
    index: i32,
) -> Option<(GenreAlbumDetailDoc, Vec<LocalTrack>)> {
    let index = usize::try_from(index).ok()?;
    let versions = state(|s| s.genre_detail_versions.get(album_id).cloned())?;
    let selected = versions.get(index)?.1.clone();
    let doc = genre_detail_doc(album_id, versions.as_slice(), index);
    Some((doc, selected))
}

/// The header for ONE version — recomputed per version (the Slint recomputes
/// title / artist / info-line / quality in `apply_album_version`, because two
/// copies of the same album differ exactly in those). `directoryPath` and
/// `folderCount` come from the loaded card when it is in a mounted page; a
/// deep link leaves them empty, as the grid does.
fn album_header(id: &str, tracks: &[LocalTrack]) -> AlbumRow {
    let first = &tracks[0];
    let artist_of = |t: &LocalTrack| t.album_artist.clone().unwrap_or_else(|| t.artist.clone());
    let lead = artist_of(first);
    let artist = if tracks.iter().all(|t| artist_of(t) == lead) {
        lead.clone()
    } else {
        qbz_i18n::t("Various Artists")
    };
    // Distinct TRACK artists, first-appearance order — the "+N more artists"
    // expander (spec §B2). Raw `artist`, not `album_artist`: a compilation's
    // track artists are the list's whole point. QML splits this on ",".
    let mut all_artists: Vec<&str> = Vec::new();
    for t in tracks {
        let a = t.artist.trim();
        if !a.is_empty() && !all_artists.contains(&a) {
            all_artists.push(a);
        }
    }
    let best = tracks
        .iter()
        .max_by_key(|track| version_rank(track))
        .unwrap_or(first);
    let best_source_raw = badge_source_raw(best.source.as_deref());
    let best_source = if best_source_raw.is_empty() {
        badge_source(best.source.as_deref())
    } else {
        best_source_raw
    };
    let card = state(|s| {
        s.albums
            .iter()
            .chain(s.folders.iter())
            .find(|a| a.id == id)
            .cloned()
    });
    let sources = card.as_ref().map(|c| c.sources.clone()).unwrap_or_else(|| {
        let mut values = Vec::new();
        for track in state(|s| {
            s.album_versions
                .iter()
                .filter_map(|(_, tracks)| tracks.first().cloned())
                .collect::<Vec<_>>()
        }) {
            let raw = badge_source_raw(track.source.as_deref());
            let source = if raw.is_empty() {
                badge_source(track.source.as_deref())
            } else {
                raw
            };
            if !values.contains(&source) {
                values.push(source);
            }
        }
        values
    });
    let favoriteable = album_favorite_source(&sources).is_some();
    AlbumRow {
        id: id.to_string(),
        title: first.album_group_title.clone(),
        artist,
        all_artists: all_artists.join(", "),
        artists: {
            let names = all_artists.to_vec();
            let aliases = crate::local_artist_match::build_artist_family_aliases(&names);
            crate::local_artist_match::album_credit_names(&lead, &all_artists.join(","), &aliases)
        },
        year: first.year.map(|y| y.to_string()).unwrap_or_default(),
        track_count: tracks.len() as u32,
        duration: total_duration(tracks.iter().map(|t| t.duration_secs).sum()),
        quality_tier: tier_of(&best.format, best.bit_depth, best.sample_rate).into(),
        quality_detail: crate::local_rows::detail_of(
            &best.format,
            best.bit_depth,
            best.sample_rate,
        ),
        format: best.format.to_string(),
        media_variants: vec![AlbumMediaVariant {
            quality_tier: tier_of(&best.format, best.bit_depth, best.sample_rate).into(),
            format: best.format.to_string(),
            source: best_source,
        }],
        genres: {
            let mut values = tracks
                .iter()
                .flat_map(|track| track.genres.iter().cloned())
                .collect::<Vec<_>>();
            values.sort_by_key(|genre| genre.to_lowercase());
            values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            values
        },
        art_key: album_key(id),
        source: badge_source(first.source.as_deref()),
        sources,
        source_raw: badge_source_raw(first.source.as_deref()),
        directory_path: card
            .as_ref()
            .map(|c| c.directory_path.clone())
            .unwrap_or_default(),
        folder_count: card.map(|c| c.folder_count).unwrap_or(0),
        is_favorite: favoriteable && crate::library_qt::is_local_favorite("album", id),
        favoriteable,
    }
}

/// Toggle a Local Library album using the card's denormalized display
/// snapshot. This is the Qt port of Slint's `toggle_album_favorite`, with the
/// source guard made explicit so a Qobuz-offline row cannot write a duplicate
/// local-library favorite. Media-server rows are valid snapshots here.
pub(crate) fn toggle_album_favorite(
    id: String,
    title: String,
    artist: String,
    artwork_url: String,
    sources_json: String,
) {
    let sources: Vec<String> = serde_json::from_str(&sources_json).unwrap_or_default();
    let Some(source) = album_favorite_source(&sources) else {
        log::warn!("[qbz-qt] local favorite refused for unsupported album {id}");
        return;
    };
    let item = LocalFavItem {
        kind: "album".to_string(),
        id: id.clone(),
        title,
        subtitle: artist.clone(),
        artwork_url,
        artist,
        source: source.to_string(),
        favorited_at: 0,
    };
    let feed_item = item.clone();
    crate::spawn(async move {
        let id_for_write = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let current = crate::library_qt::is_local_favorite("album", &id_for_write);
            (
                crate::library_qt::toggle_local_favorite_snapshot(item),
                current,
            )
        })
        .await;
        let favorite = match result {
            Ok((Some(value), _)) => value,
            Ok((None, current)) => {
                log::warn!("[qbz-qt] local favorite store unavailable for album {id}");
                current
            }
            Err(error) => {
                log::error!("[qbz-qt] local favorite worker failed for album {id}: {error}");
                crate::library_qt::is_local_favorite("album", &id)
            }
        };
        let membership_changed = if favorite {
            crate::library_qt::insert_local_favorite_row(&feed_item)
        } else {
            crate::library_qt::set_feed_favorite("album", &id, false);
            crate::library_qt::remove_local_favorite_row("album", &id)
        };
        state(|local| {
            for row in local.albums.iter_mut().chain(local.folders.iter_mut()) {
                if row.id == id {
                    row.is_favorite = favorite;
                }
            }
        });
        crate::emit_library_favorite("album", &id, favorite);
        if membership_changed {
            crate::publish_library_document();
        }
        ui(move |mut bridge| {
            bridge
                .as_mut()
                .local_album_favorite_changed(QString::from(id.as_str()), favorite);
        });
    });
}

// ---------------------------------------------------------------------------
// The OPEN album's track cache (the Slint's `current_album_version_tracks` /
// `current_album_disc_tracks` / `album_version_dir`)
// ---------------------------------------------------------------------------

/// The selected version's tracks (play / enqueue / the unwired actions).
pub fn current_version_tracks() -> Vec<LocalTrack> {
    state(|s| {
        s.album_versions
            .get(s.album_version_index)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    })
}

/// The selected version's SOURCE DIRECTORY key — what a tag editor would edit.
pub fn current_version_dir() -> String {
    state(|s| {
        s.album_versions
            .get(s.album_version_index)
            .map(|(dir, _)| dir.clone())
            .unwrap_or_default()
    })
}

/// One disc of the selected version, in the upstream (disc, track) order.
/// `disc_number` defaults to 1 — exactly how the header number is stamped.
fn current_disc_tracks(disc: i32) -> Vec<LocalTrack> {
    current_version_tracks()
        .into_iter()
        .filter(|t| t.disc_number.unwrap_or(1) as i32 == disc)
        .collect()
}

/// One disc from one concurrently expanded Genres album. Unlike the routed
/// AlbumView cache, this is keyed by album id: opening/scrolling a second
/// details block must not retarget the first one's menu.
fn genre_disc_tracks(album_id: &str, disc: i32) -> Vec<LocalTrack> {
    state(|s| {
        s.genre_detail_raw
            .get(album_id)
            .into_iter()
            .flatten()
            .filter(|t| t.disc_number.unwrap_or(1) as i32 == disc)
            .cloned()
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Version picker: switch the shown copy in place (no DB round-trip).
pub fn select_version(index: i32) {
    if index < 0 {
        return;
    }
    let index = index as usize;
    let id = state(|s| {
        if index >= s.album_versions.len() || index == s.album_version_index {
            return None;
        }
        s.album_version_index = index;
        Some(s.album_id.clone())
    });
    let Some(id) = id.filter(|id| !id.is_empty()) else {
        return;
    };
    publish_doc(version_doc(&id, index));
}

/// Per-disc "Disc N" header menu. `action` is the QML CardMenu's own vocabulary
/// ("play" | "next" | "later" | "queue") over THIS disc's tracks only, reusing
/// the same queue ops as the header play button and the per-row menu.
pub fn disc_action(disc: i32, action: String) {
    dispatch_disc_action(
        "local album".to_string(),
        current_disc_tracks(disc),
        disc,
        action,
    );
}

/// The same four actions for a disc header inside Genres Details. It cannot
/// call `disc_action`: that function intentionally reads the ONE routed
/// AlbumView version, while Genres may keep 32 expanded albums alive.
pub fn genre_disc_action(album_id: String, disc: i32, action: String) {
    let tracks = genre_disc_tracks(&album_id, disc);
    dispatch_disc_action(format!("genres album {album_id}"), tracks, disc, action);
}

/// Album and row actions for one selected Genres version. Unlike the generic
/// album action this never re-fetches the logical album (which would include
/// every physical copy and duplicate the queue).
pub fn genre_album_action(album_id: String, action: String, track_id: Option<i64>) {
    let tracks = state(|s| {
        s.genre_detail_raw
            .get(&album_id)
            .cloned()
            .unwrap_or_default()
    });
    if tracks.is_empty() {
        log::debug!("[qbz-qt] genres album {album_id}: no selected rows for '{action}'");
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        match action.as_str() {
            "play" | "shuffle" => {
                let start = track_id
                    .and_then(|id| tracks.iter().position(|track| track.id == id))
                    .unwrap_or(0);
                crate::local_playback::play_rows(&runtime, tracks, start, action == "shuffle")
                    .await;
            }
            "next" | "later" | "queue" => enqueue_rows(&runtime, tracks, &action).await,
            other => {
                log::debug!("[qbz-qt] genres album {album_id}: unhandled album action '{other}'")
            }
        }
    });
}

/// Play the selected physical copy in the routed AlbumView. Re-querying the
/// logical album here would concatenate every copy and would also bypass the
/// source funnel used to open this view.
pub fn selected_album_action(action: String, track_id: Option<i64>) {
    let tracks = current_version_tracks();
    if tracks.is_empty() {
        log::debug!("[qbz-qt] local album: no selected rows for '{action}'");
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        match action.as_str() {
            "play" | "shuffle" => {
                let start = track_id
                    .and_then(|id| tracks.iter().position(|track| track.id == id))
                    .unwrap_or(0);
                crate::local_playback::play_rows(&runtime, tracks, start, action == "shuffle")
                    .await;
            }
            "next" | "later" | "queue" => enqueue_rows(&runtime, tracks, &action).await,
            other => log::debug!("[qbz-qt] local album: unhandled selected action '{other}'"),
        }
    });
}

fn dispatch_disc_action(context: String, tracks: Vec<LocalTrack>, disc: i32, action: String) {
    if tracks.is_empty() {
        log::debug!("[qbz-qt] {context}: disc {disc} has no tracks for '{action}'");
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move {
        match action.as_str() {
            "play" | "shuffle" => {
                crate::local_playback::play_rows(&runtime, tracks, 0, action == "shuffle").await
            }
            "next" | "later" | "queue" => enqueue_rows(&runtime, tracks, &action).await,
            other => log::debug!("[qbz-qt] {context}: unhandled disc action '{other}'"),
        }
    });
}

/// "Go to artist" on a local/Plex album: those artists have no catalog id, so
/// the route is a NAME the Local Library view consumes on its Artists tab
/// (`consumePendingArtist`). Cleared by `clear_pending_artist` once applied.
pub fn open_artist_by_name(name: String) {
    if name.trim().is_empty() {
        return;
    }
    ui(move |mut b| {
        b.as_mut()
            .set_local_pending_artist(QString::from(name.as_str()));
    });
}

/// The view consumed the pending name — release it so the same artist can be
/// routed to twice in a row (the property change IS the trigger).
pub fn clear_pending_artist() {
    ui(|mut b| b.as_mut().set_local_pending_artist(QString::from("")));
}

/// Route the Local Library view to a TAB, optionally pre-filtered.
///
/// Used by the cortinilla's local "View more" links: those leave the search
/// surface entirely and land the user on the matching Local Library tab,
/// rather than on a Qobuz results page that has no local content at all.
///
/// The tab is QML-owned state (there is no `local_tab` property on this
/// bridge), so this is a pending route the view consumes, exactly like
/// `local_pending_artist` above. `query` is applied only by the tracks tab —
/// the albums and artists tabs have no search box of their own.
pub fn set_pending_route(tab: &str, query: &str) {
    let json = serde_json::json!({ "tab": tab, "query": query }).to_string();
    ui(move |mut b| {
        b.as_mut()
            .set_local_pending_route(QString::from(json.as_str()));
    });
}

/// The view applied the pending route — release it so the same route can fire
/// twice in a row.
pub fn clear_pending_route() {
    ui(|mut b| b.as_mut().set_local_pending_route(QString::from("")));
}

/// Mirror `AppearanceState.local-library-track-artwork` (ui_prefs.json,
/// default OFF — per-row covers on a 16K-track list are the freeze surface)
/// onto the bridge so the Local Library view can gate its track artwork.
pub fn publish_track_artwork() {
    let on = crate::settings_qt::pref_bool("local_library_track_artwork", false);
    ui(move |mut b| b.as_mut().set_local_track_artwork(on));
}

/// Album header: open the app-wide editor for the selected physical
/// version. The controller snapshots its database rows, and QML receives row
/// ids rather than file paths so a stale or modified draft cannot retarget a
/// write.
pub fn edit_tags(id: String) {
    crate::tag_editor_qt::open(id);
}

/// Album header ＋: the picker over the SELECTED version's tracks, in LOCAL
/// MODE (the Slint's `playlist_picker::open_for_ids(.., local = true)`).
///
/// The refs are built by `local_picker_ref_for_track`, never by hand: a Plex
/// row rides `plex:<rating key>` and everything else its library row id, and
/// the `Payload::LocalRefs` variant is what keeps either of them from reaching
/// a Qobuz endpoint, where the same number means a different track.
pub fn add_to_playlist(id: String) {
    let tracks = current_version_tracks();
    if tracks.is_empty() {
        log::warn!("[qbz-qt] local album add-to-playlist: no version open for album '{id}'");
        return;
    }
    open_picker_for_rows(&tracks);
}

/// The shared tail of every local add-to-playlist entry: source-aware refs
/// (`local_picker_ref_for_track` — Plex and Jellyfin/Subsonic ride their
/// source-native keys, everything else its library row id), then the picker.
pub(crate) fn open_picker_for_rows(tracks: &[qbz_library::LocalTrack]) {
    let refs: Vec<String> = tracks
        .iter()
        .map(crate::local_playlist_qt::local_picker_ref_for_track)
        .collect();
    if refs.is_empty() {
        return;
    }
    crate::playlist_picker_qt::open_for_local_refs(&crate::app(), refs);
}

/// Album header 📼: ONE `album` payload (source "local", the album cover, no
/// year) plus the SELECTED version's track count, then the Mixtape/Collection
/// picker — 1:1 with `LocalAlbumActions::on_add_to_mixtape`
/// (`qbz/src/main.rs:18714-18737`).
///
/// Title and artist come from `album_header`, the same function that produced
/// the header the user is looking at, so a version switch is reflected (the
/// Slint reads `LocalAlbumState`, which `apply_album_version` recomputes for
/// exactly that reason).
///
/// Deviation, deliberate: the reference does not check the track list and lets
/// `track_count` fall to `None`; here an empty list means no version is open at
/// all, and `album_header` indexes `tracks[0]`, so it is refused with a log
/// instead of panicking.
pub fn add_to_mixtape(id: String) {
    if id.is_empty() {
        return;
    }
    let tracks = current_version_tracks();
    if tracks.is_empty() {
        log::warn!("[qbz-qt] local album add-to-mixtape: no open version for '{id}', ignored");
        return;
    }
    let row = album_header(&id, &tracks);
    let item = crate::myqbz_add_qt::AddItem {
        item_type: "album".into(),
        source: "local".into(),
        source_item_id: id,
        title: row.title,
        subtitle: (!row.artist.is_empty()).then_some(row.artist),
        // THE COVER, which this payload used to drop on the floor.
        //
        // `mixtape_collection_items.artwork_url` is a SNAPSHOT written once at
        // add time — the grid tile and the hero mosaic read that column and
        // stop, so a row stored without one is coverless for the life of the
        // row. Measured in the owner's live library.db on 2026-08-22: EIGHT
        // rows with an empty artwork_url, every one of them
        // `item_type='album' source='local'`, while every Qobuz album row
        // carried a url. This call site was the only place that could produce
        // them.
        //
        // The portable reference is safe to persist: local covers are encoded
        // file URLs, Plex remains a server-relative thumb, and Jellyfin /
        // Subsonic tokens carry their source prefix without credentials. The
        // collection layer wins because this payload represents an ALBUM, not
        // one disc/track.
        artwork_url: tracks.iter().find_map(|track| {
            crate::local_rows::portable_artwork_ref(track, crate::local_rows::ArtworkScope::Album)
        }),
        year: None,
        track_count: Some(tracks.len() as i32),
    };
    crate::myqbz_add_qt::open_items(vec![item]);
}

// ---------------------------------------------------------------------------
// Publish + queue helpers
// ---------------------------------------------------------------------------

fn publish_doc(doc: Option<AlbumDetailDoc>) {
    let json = doc.map(|d| to_json(&d)).unwrap_or_default();
    ui(move |mut b| {
        b.as_mut()
            .set_local_album_json(QString::from(json.as_str()));
        b.as_mut().set_local_album_loading(false);
    });
}

/// "Play next" / "Play later" / "Add to queue" over a track SUBSET — the same
/// core helpers `local_playback::enqueue` uses, just without the id round-trip.
///
/// The shared enqueue seam runs before any core mutation. While QConnect is
/// enabled it drops this local-only batch, raises the single counted notice,
/// and leaves the existing queue untouched.
async fn enqueue_rows(runtime: &Runtime, tracks: Vec<LocalTrack>, mode: &str) {
    let tracks = tokio::task::spawn_blocking(move || {
        let mut tracks = tracks;
        fill_missing_covers(&mut tracks);
        tracks
    })
    .await
    .unwrap_or_default();
    let queue: Vec<QueueTrack> = tracks.iter().map(local_queue_track).collect();
    let queue = crate::playback_qt::stamped(queue, None);
    if queue.is_empty() {
        return;
    }
    let Some(_owner_action) = crate::playback_qt::begin_owner_action() else {
        return;
    };
    match mode {
        // Reversed so a multi-track insert at the cursor keeps its order.
        "next" => {
            for t in queue.into_iter().rev() {
                runtime.core().add_track_next(t).await;
            }
        }
        "later" => {
            for t in queue {
                runtime.core().add_track_later(t).await;
            }
        }
        _ => runtime.core().add_tracks(queue).await,
    }
    crate::playback_qt::publish_queue(runtime).await;
}

#[cfg(test)]
mod version_tests {
    use super::*;

    fn copy(key: &str, source: &str, title: &str, count: usize, depth: u32) -> Vec<LocalTrack> {
        (0..count)
            .map(|index| LocalTrack {
                id: index as i64 + 1,
                file_path: format!("{key}/{index}.flac"),
                title: format!("Track {index}"),
                artist: "Artist".to_string(),
                album: title.to_string(),
                album_artist: Some("Artist".to_string()),
                album_group_key: key.to_string(),
                album_group_title: title.to_string(),
                track_number: Some(index as u32 + 1),
                disc_number: Some(1),
                format: AudioFormat::Flac,
                bit_depth: Some(depth),
                sample_rate: if depth > 16 { 96_000.0 } else { 44_100.0 },
                source: Some(source.to_string()),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn versions_sort_by_quality_then_track_count_and_stay_distinct() {
        let mut tracks = copy("cd-short", "plex", "Album", 10, 16);
        tracks.extend(copy(
            "hires-short",
            "jellyfin",
            "Album (2012 Remaster)",
            9,
            24,
        ));
        tracks.extend(copy(
            "hires-deluxe",
            "subsonic",
            "Album - Deluxe Edition",
            12,
            24,
        ));
        let versions = split_versions(tracks);
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].0, "hires-deluxe");
        assert_eq!(versions[1].0, "hires-short");
        assert_eq!(versions[2].0, "cd-short");

        let info = version_info(&versions[0].1);
        assert_eq!(info.version, "Deluxe Edition");
        assert_eq!(info.track_count, 12);
        assert_eq!(info.source, "subsonic");
        assert!(info.quality.contains("FLAC"));
    }

    #[test]
    fn dsd_version_is_preselected_ahead_of_pcm_hires() {
        let mut tracks = copy("pcm-24-96", "user", "Rust in Peace", 9, 24);
        let mut dsd = copy("purchased-dsd", "qobuz_purchase", "Rust in Peace", 9, 1);
        for track in &mut dsd {
            track.file_path = track.file_path.replace(".flac", ".iso");
            track.format = AudioFormat::Dsd;
            track.sample_rate = 2_822_400.0;
        }
        tracks.extend(dsd);

        let versions = split_versions(tracks);
        assert_eq!(versions[0].0, "purchased-dsd");
        assert_eq!(version_info(&versions[0].1).quality, "DSD64 · DSD");
    }

    #[test]
    fn genres_detail_keeps_each_named_disc_and_its_own_cover() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "qbz-genres-eternal-cd-box-{}-{nonce}",
            std::process::id()
        ));
        let disc_one = root.join("Disc 01 - TV Series Soundtrack #01");
        let disc_two = root.join("Disc 02 - TV Series Soundtrack #02");
        std::fs::create_dir_all(&disc_one).unwrap();
        std::fs::create_dir_all(&disc_two).unwrap();
        std::fs::write(disc_one.join("cover.jpg"), b"disc-one").unwrap();
        std::fs::write(disc_two.join("folder.jpg"), b"disc-two").unwrap();

        let track = |id, disc, folder: &std::path::Path| LocalTrack {
            id,
            file_path: folder.join("01.flac").to_string_lossy().into_owned(),
            title: format!("Movement {disc}"),
            artist: "Seiji Yokoyama".to_string(),
            album: "Saint Seiya Eternal CD-Box".to_string(),
            album_artist: Some("Seiji Yokoyama".to_string()),
            album_group_key: root.to_string_lossy().into_owned(),
            album_group_title: "Saint Seiya Eternal CD-Box".to_string(),
            track_number: Some(1),
            disc_number: Some(disc),
            format: AudioFormat::Flac,
            bit_depth: Some(16),
            sample_rate: 44_100.0,
            ..Default::default()
        };
        let versions = vec![(
            root.to_string_lossy().into_owned(),
            vec![track(1, 1, &disc_one), track(2, 2, &disc_two)],
        )];
        let doc = genre_detail_doc("test:eternal-cd-box", &versions, 0);

        assert_eq!(doc.tracks.len(), 2);
        assert_eq!(doc.discs.len(), 2);
        assert_eq!(doc.versions.len(), 1);
        assert_eq!(doc.version_index, 0);
        assert_eq!(doc.discs[0].title, "TV Series Soundtrack #01");
        assert_eq!(doc.discs[1].title, "TV Series Soundtrack #02");
        assert!(!doc.discs[0].cover.is_empty());
        assert!(!doc.discs[1].cover.is_empty());
        assert_ne!(doc.discs[0].cover, doc.discs[1].cover);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn remote_disc_headers_keep_each_sources_track_art_token() {
        let track = |id, disc, token: &str| LocalTrack {
            id,
            file_path: format!("remote-{id}"),
            title: format!("Movement {disc}"),
            artist: "Seiji Yokoyama".to_string(),
            album: "Saint Seiya Eternal CD-Box".to_string(),
            album_group_key: "subsonic:eternal-box".to_string(),
            album_group_title: "Saint Seiya Eternal CD-Box".to_string(),
            track_number: Some(1),
            disc_number: Some(disc),
            artwork_path: Some(token.to_string()),
            source: Some("subsonic".to_string()),
            format: AudioFormat::Flac,
            ..Default::default()
        };
        let versions = vec![(
            "subsonic:eternal-box".to_string(),
            vec![
                track(910_001, 1, "dc-disc-one"),
                track(910_002, 2, "dc-disc-two"),
            ],
        )];
        let doc = genre_detail_doc("test:remote-disc-art", &versions, 0);

        assert_eq!(doc.discs.len(), 2);
        assert_eq!(doc.discs[0].art_key, "track:910001");
        assert_eq!(doc.discs[1].art_key, "track:910002");
        with_art(|art| {
            assert_eq!(art["track:910001"].1, "dc-disc-one");
            assert_eq!(art["track:910002"].1, "dc-disc-two");
            art.remove("track:910001");
            art.remove("track:910002");
            art.remove(&doc.art_key);
        });
    }

    #[test]
    fn selected_version_owns_art_and_coverless_version_falls_back() {
        let mut jellyfin = copy("jf-hires", "jellyfin", "Album", 2, 24);
        let mut plex = copy("plex-cd", "plex", "Album", 2, 16);
        plex[0].artwork_path = Some("/library/metadata/album/thumb".to_string());
        let versions = vec![
            ("jf-hires".to_string(), jellyfin.clone()),
            ("plex-cd".to_string(), plex.clone()),
        ];

        let fallback = version_artwork_ref(&jellyfin, &versions).expect("Plex fallback");
        assert_eq!(fallback.0.as_deref(), Some("plex"));
        assert_eq!(fallback.1, "/library/metadata/album/thumb");

        jellyfin[1].artwork_path = Some("jf:item:cover".to_string());
        let selected = version_artwork_ref(&jellyfin, &versions).expect("selected cover");
        assert_eq!(selected.0.as_deref(), Some("jellyfin"));
        assert_eq!(selected.1, "jf:item:cover");

        assert_ne!(
            album_version_art_key("logical:album", 0),
            album_version_art_key("logical:album", 1)
        );
    }

    #[test]
    fn genres_version_selection_returns_one_expanded_albums_copy() {
        let album = "test:genres-version-picker";
        let mut cd = copy("genres-cd", "plex", "Album", 2, 16);
        let mut hires = copy("genres-hires", "jellyfin", "Album (Remastered)", 3, 24);
        for track in &mut cd {
            track.id += 100;
        }
        for track in &mut hires {
            track.id += 200;
        }
        let versions = vec![
            ("genres-hires".to_string(), hires.clone()),
            ("genres-cd".to_string(), cd.clone()),
        ];
        state(|s| {
            s.genre_detail_raw.insert(album.to_string(), hires);
            s.genre_detail_versions
                .insert(album.to_string(), std::sync::Arc::new(versions.clone()));
        });

        let (doc, selected) = genre_version_selection(album, 1).expect("second version");
        assert_eq!(doc.version_index, 1);
        assert_eq!(doc.versions.len(), 2);
        assert_eq!(doc.tracks.len(), 2);
        assert_eq!(
            selected.iter().map(|track| track.id).collect::<Vec<_>>(),
            cd.iter().map(|track| track.id).collect::<Vec<_>>()
        );

        let (back, selected) = genre_version_selection(album, 0).expect("first version again");
        assert_eq!(back.version_index, 0);
        assert_eq!(back.versions.len(), 2);
        assert_eq!(back.tracks.len(), 3);
        assert_eq!(
            selected.iter().map(|track| track.id).collect::<Vec<_>>(),
            versions[0]
                .1
                .iter()
                .map(|track| track.id)
                .collect::<Vec<_>>()
        );
        assert!(genre_version_selection(album, 2).is_none());

        state(|s| {
            s.genre_detail_raw.remove(album);
            s.genre_detail_versions.remove(album);
        });
    }

    #[test]
    fn genres_disc_menu_targets_its_album_and_disc_only() {
        let album = "test:genres-disc-actions";
        let rows = vec![
            LocalTrack {
                id: 11,
                disc_number: Some(1),
                ..Default::default()
            },
            LocalTrack {
                id: 21,
                disc_number: Some(2),
                ..Default::default()
            },
            LocalTrack {
                id: 22,
                disc_number: Some(2),
                ..Default::default()
            },
        ];
        state(|s| {
            s.genre_detail_raw.insert(album.to_string(), rows);
        });

        let selected = genre_disc_tracks(album, 2);
        assert_eq!(selected.iter().map(|t| t.id).collect::<Vec<_>>(), [21, 22]);
        assert!(genre_disc_tracks("test:another-album", 2).is_empty());

        state(|s| {
            s.genre_detail_raw.remove(album);
        });
    }
}
