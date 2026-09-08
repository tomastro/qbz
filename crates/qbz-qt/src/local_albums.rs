//! Local Library queries: albums, folder-albums, artists, the paginated
//! Tracks tab, the tab badges and the album-detail pane.
//!
//! Split out of `local_library_qt.rs` (phase-24 modularization). Every query
//! is a BLOCKING body (the bridge wraps it in `spawn_blocking`) and every one
//! is Plex-aware:
//!
//!  - Albums / badges: `get_albums_metadata_page(..., plex_cache_path, ...)`
//!    ATTACHes the Plex cache DB, so the returned page is the local+Plex
//!    UNION (sort, search and pagination apply to the union as one set).
//!  - Tracks: each enabled source contributes one bounded candidate page.
//!    A stable global merge advances only the offsets actually consumed.
//!  - Album detail: a `plex:`-prefixed group key is served from the Plex
//!    cache instead of `library.db`.

use std::collections::{HashMap, HashSet};

use qbz_library::album_grouping::AlbumGroupMode;
#[cfg(test)]
use qbz_library::AudioFormat;
use qbz_library::{AlbumTrackEvidence, LocalAlbum, LocalTrack};

use crate::local_album_actions::AlbumDetailDoc;
use crate::local_artist_match::{
    album_credit_names, album_matches_artist_with_aliases, build_artist_family_aliases,
    merge_artists, normalize_artist, AlbumCredit, ArtistInput,
};
use crate::local_rows::{
    album_media_variants, artist_key, badge_source, map_album, map_album_with_artists, map_track,
    media_quality_rank, tier_of, AlbumMediaVariant, AlbumRow, ArtistRow, LocalCounts, TrackRow,
};
use crate::local_state::{
    commit_tracks_page, group_mode, state, tracks_generation, with_art, with_db,
    TrackSourceOffsets, TracksLoadRequest, TRACKS_PAGE,
};

/// Albums tab: the FULL metadata/folder-grouped set (local + Plex) in one
/// page. Search, sort and grouping derive QML-side over the parsed array.
pub fn load_albums_blocking() -> Result<Vec<AlbumRow>, String> {
    // TIMED, in three segments, because this document is republished on EVERY
    // navigation (5 times in a 37-second screencast) and there was no
    // instrument on the path at all — so "the grid got slower" could only be
    // argued about. The segments are the three things that can actually cost:
    // the SQL (which now ATTACHes two mirrors and UNIONs three sources), the
    // row mapping (which resolves an ArtRef per row), and the JSON the bridge
    // hands to QML.
    let t0 = std::time::Instant::now();
    let mode = group_mode();
    let plex_path = crate::local_plex::cache_db_path();
    // The SHARED remote mirror, and which of its sources may show. Both gates
    // matter: the path short-circuits the ATTACH for a user with no media
    // server, and the words are what make the master toggle actually remove a
    // server's rows from the grid (the mirror holds them all).
    let remote_path = crate::media_servers_qt::remote_cache_path();
    let remote_words = crate::media_servers_qt::configured_words();
    let exclude_network = crate::offline_fwd::exclude_network_folders_now();
    let page = with_db(|db| {
        db.get_albums_metadata_page(
            0,
            100_000,
            None,
            "artist",
            "asc",
            /* include_qobuz_downloads */ true,
            exclude_network,
            plex_path.as_deref(),
            remote_path.as_deref(),
            &remote_words,
            mode,
        )
    })
    .ok_or_else(|| "local library not available".to_string())?;
    let t_sql = t0.elapsed();
    let identity_started = std::time::Instant::now();
    let source_album_count = page.albums.len();
    // Preserve the physical tuples before coalescing chooses one visible
    // representative. A logical card must match DSD when any of its versions
    // is DSD, even when another copy owns the normal unfiltered badge.
    let physical_media = page
        .albums
        .iter()
        .map(|album| (album.id.clone(), album_media_variants(album)))
        .collect::<HashMap<_, _>>();
    let (albums, version_ids) = coalesce_album_versions(page.albums);
    let t_identity = identity_started.elapsed();
    let total = albums.len() as u64;
    let n = albums.len();
    let t1 = std::time::Instant::now();
    let mut family_names = albums
        .iter()
        .map(|album| album.artist.as_str())
        .collect::<Vec<_>>();
    for album in &albums {
        family_names.extend(album.all_artists.split(',').filter(|name| !name.is_empty()));
    }
    let aliases = build_artist_family_aliases(&family_names);
    let rows = with_art(|art| {
        albums
            .into_iter()
            .map(|a| {
                let media_variants = logical_album_media_variants(
                    &a,
                    version_ids.get(&a.id).map(Vec::as_slice),
                    &physical_media,
                );
                let artists = album_credit_names(&a.artist, &a.all_artists, &aliases);
                let mut row = map_album_with_artists(a, art, artists);
                row.media_variants = media_variants;
                row
            })
            .collect::<Vec<AlbumRow>>()
    });
    log::info!(
        "[qbz-qt][perf] albums load: {source_album_count} source rows -> {n} logical rows — sql {t_sql:?}, identity {t_identity:?}, map {:?} (plex={} remote={:?})",
        t1.elapsed(),
        plex_path.is_some(),
        remote_words,
    );
    state(|s| {
        s.counts.albums = total as i64;
        s.albums = rows.clone();
        s.album_version_ids = version_ids;
    });
    Ok(rows)
}

/// Resolve which persisted local-favorite album ids still belong to the
/// active Local Library.  A favorite is only a display snapshot; it is not
/// proof that the underlying local/media-server album still exists.
///
/// The common case checks the handful of favorite ids on one DB connection.
/// A `logical:*` id represents several physical copies and can only be
/// reconstructed by the same coalescing pass as the Albums tab, so that rare
/// arm deliberately runs the full loader once rather than guessing from a
/// stale in-memory version map.
pub fn existing_favorite_album_ids_blocking(
    candidates: Vec<(String, String)>,
) -> Result<HashSet<String>, String> {
    existing_favorite_album_sources_blocking(candidates).map(|rows| rows.into_keys().collect())
}

/// Preserve the badge sources from the rows already read by the availability
/// check. A logical album key cannot identify its physical sources by itself.
pub(crate) fn existing_favorite_album_sources_blocking(
    candidates: Vec<(String, String)>,
) -> Result<HashMap<String, Vec<String>>, String> {
    if candidates.is_empty() {
        return Ok(HashMap::new());
    }
    if candidates.iter().any(|(id, _)| id.starts_with("logical:")) {
        return load_albums_blocking()
            .map(|rows| rows.into_iter().map(|row| (row.id, row.sources)).collect());
    }

    let mut existing = HashMap::new();
    for (id, _) in candidates.iter().filter(|(_, source)| source == "plex") {
        if crate::local_plex::is_configured() {
            let tracks = crate::local_plex::album_tracks(id);
            if !tracks.is_empty() {
                existing.insert(
                    id.clone(),
                    crate::local_rows::source_badges(
                        tracks.iter().map(|track| track.source.as_deref()),
                    ),
                );
            }
        }
    }

    for (id, _) in candidates
        .iter()
        .filter(|(_, source)| matches!(source.as_str(), "jellyfin" | "subsonic"))
    {
        if let Some(tracks) =
            crate::media_servers_qt::album_tracks(id).filter(|tracks| !tracks.is_empty())
        {
            existing.insert(
                id.clone(),
                crate::local_rows::source_badges(
                    tracks.iter().map(|track| track.source.as_deref()),
                ),
            );
        }
    }

    let local_ids = candidates
        .iter()
        .filter(|(_, source)| source == "local")
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if local_ids.is_empty() {
        return Ok(existing);
    }
    let mode = group_mode();
    let found = with_db(|db| {
        let mut found = HashMap::new();
        for id in &local_ids {
            let tracks = match mode {
                AlbumGroupMode::Metadata => {
                    let metadata = db.get_album_tracks_metadata(id)?;
                    if metadata.is_empty() {
                        db.get_album_tracks(id)?
                    } else {
                        metadata
                    }
                }
                AlbumGroupMode::Folder => db.get_album_tracks(id)?,
            };
            if !tracks.is_empty() {
                found.insert(
                    id.clone(),
                    crate::local_rows::source_badges(
                        tracks.iter().map(|track| track.source.as_deref()),
                    ),
                );
            }
        }
        Ok(found)
    })
    .ok_or_else(|| "local library not available".to_string())?;
    existing.extend(found);
    Ok(existing)
}

/// Fold source copies into one visible album only when their content strongly
/// agrees. Title/artist merely form a candidate bucket; the actual authority
/// is an at-least-80% one-to-one track-title + duration overlap (or the one
/// track itself for a single). The map is ephemeral and rebuilt from the
/// authoritative caches, so a later correction can split an association.
fn coalesce_album_versions(
    mut albums: Vec<LocalAlbum>,
) -> (Vec<LocalAlbum>, HashMap<String, Vec<String>>) {
    albums.sort_by(|left, right| left.id.cmp(&right.id));
    let mut buckets = std::collections::BTreeMap::<String, Vec<LocalAlbum>>::new();
    let mut singles = Vec::new();
    for mut album in albums {
        album.sources.sort_by(|left, right| {
            source_order(left)
                .cmp(&source_order(right))
                .then_with(|| left.cmp(right))
        });
        album.sources.dedup();
        let title = logical_title(&album.title);
        let artist = normalize_artist(&album.artist);
        if title.is_empty()
            || artist.is_empty()
            || title == "unknown album"
            || artist == "unknown artist"
            || album.identity_tracks.is_empty()
        {
            singles.push(album);
        } else {
            buckets
                .entry(format!("{artist}\u{1f}{title}"))
                .or_default()
                .push(album);
        }
    }

    let mut groups = Vec::<Vec<LocalAlbum>>::new();
    for bucket in buckets.into_values() {
        let mut bucket_groups = Vec::<Vec<LocalAlbum>>::new();
        for album in bucket {
            if let Some(group) = bucket_groups
                .iter_mut()
                .find(|group| group.iter().all(|other| copies_match(other, &album)))
            {
                group.push(album);
            } else {
                bucket_groups.push(vec![album]);
            }
        }
        groups.extend(bucket_groups);
    }
    groups.extend(singles.into_iter().map(|album| vec![album]));

    let mut version_ids = HashMap::new();
    let mut merged = groups
        .into_iter()
        .map(|mut group| {
            group.sort_by(|left, right| {
                album_quality_rank(right)
                    .cmp(&album_quality_rank(left))
                    .then_with(|| left.id.cmp(&right.id))
            });
            let ids = group
                .iter()
                .map(|album| album.id.clone())
                .collect::<Vec<_>>();
            let mut genres = group
                .iter()
                .flat_map(|album| album.genres.iter().cloned())
                .collect::<Vec<_>>();
            genres.sort_by_key(|genre| genre.to_lowercase());
            genres.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
            let mut all_artists = group
                .iter()
                .flat_map(|album| {
                    std::iter::once(album.artist.clone()).chain(
                        album
                            .all_artists
                            .split(',')
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_string),
                    )
                })
                .collect::<Vec<_>>();
            all_artists.sort_by_key(|artist| normalize_artist(artist));
            all_artists.dedup_by(|left, right| normalize_artist(left) == normalize_artist(right));
            // `group` is already best-audio-first. Pick the best-ranked copy
            // that actually has artwork, but keep its provenance separate
            // from the representative audio source. A coverless Jellyfin
            // hi-res copy must not erase the Plex cover on the logical card.
            let best_artwork = group.iter().find_map(|album| {
                album
                    .artwork_path
                    .as_ref()
                    .filter(|path| !path.is_empty())
                    .map(|path| {
                        (
                            path.clone(),
                            album
                                .artwork_source
                                .clone()
                                .unwrap_or_else(|| album.source.clone()),
                        )
                    })
            });
            let mut representative = group.remove(0);
            representative.genres = genres;
            representative.all_artists = all_artists.join(",");
            if let Some((path, source)) = best_artwork {
                representative.artwork_path = Some(path);
                representative.artwork_source = Some(source);
            }
            if !group.is_empty() {
                let logical_id = logical_album_id(&ids);
                let mut source_words = std::iter::once(&representative)
                    .chain(group.iter())
                    .flat_map(|album| {
                        if album.sources.is_empty() {
                            vec![album.source.clone()]
                        } else {
                            album.sources.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                source_words.sort_by(|left, right| {
                    source_order(left)
                        .cmp(&source_order(right))
                        .then_with(|| left.cmp(right))
                });
                source_words.dedup();
                representative.sources = source_words;
                if let Some(plain_title) = std::iter::once(&representative)
                    .chain(group.iter())
                    .filter(|album| edition_descriptor(&album.title).is_empty())
                    .map(|album| album.title.clone())
                    .min_by_key(String::len)
                {
                    representative.title = plain_title;
                }
                representative.id = logical_id.clone();
                version_ids.insert(logical_id, ids);
            } else if representative.sources.is_empty() {
                representative.sources.push(representative.source.clone());
            }
            representative
        })
        .collect::<Vec<_>>();
    merged.sort_by(|left, right| {
        normalize_artist(&left.artist)
            .cmp(&normalize_artist(&right.artist))
            .then_with(|| logical_title(&left.title).cmp(&logical_title(&right.title)))
            .then_with(|| left.id.cmp(&right.id))
    });
    (merged, version_ids)
}

fn album_quality_rank(album: &LocalAlbum) -> (u8, u32, u64, u32) {
    let (tier, depth, sample_rate) =
        media_quality_rank(&album.format, album.bit_depth, album.sample_rate);
    (tier, depth, sample_rate, album.track_count)
}

/// Reattach every physical media tuple to the logical card selected by
/// `coalesce_album_versions`. `version_ids` is authoritative for a merged
/// card; the fallback covers a single source-native album.
fn logical_album_media_variants(
    album: &LocalAlbum,
    version_ids: Option<&[String]>,
    physical_media: &HashMap<String, Vec<AlbumMediaVariant>>,
) -> Vec<AlbumMediaVariant> {
    let mut variants = version_ids
        .into_iter()
        .flatten()
        .filter_map(|id| physical_media.get(id))
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if variants.is_empty() {
        variants = physical_media
            .get(&album.id)
            .cloned()
            .unwrap_or_else(|| album_media_variants(album));
    }
    variants.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.quality_tier.cmp(&right.quality_tier))
            .then_with(|| left.format.cmp(&right.format))
    });
    variants.dedup();
    variants
}

fn source_order(source: &str) -> u8 {
    match source {
        "user" | "local" => 0,
        "qobuz_purchase" | "qobuz_download" | "offline" => 1,
        "plex" => 2,
        "jellyfin" => 3,
        "subsonic" | "navidrome" | "gonic" | "airsonic" | "astiga" => 4,
        _ => 5,
    }
}

fn logical_album_id(ids: &[String]) -> String {
    // Deterministic FNV-1a over length-delimited source-native ids. The full
    // id vector remains in LocalState and is what resolves the click.
    let mut hash = 0xcbf29ce484222325_u64;
    let mut sorted = ids.to_vec();
    sorted.sort();
    for id in sorted {
        for byte in (id.len() as u64)
            .to_le_bytes()
            .into_iter()
            .chain(id.bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("logical:{hash:016x}")
}

fn copies_match(left: &LocalAlbum, right: &LocalAlbum) -> bool {
    let smaller = left.identity_tracks.len().min(right.identity_tracks.len());
    if smaller == 0 {
        return false;
    }
    let matched = track_overlap(&left.identity_tracks, &right.identity_tracks);
    let required = if smaller == 1 {
        1
    } else {
        3.min(smaller).max((smaller * 4 + 4) / 5)
    };
    matched >= required
}

fn track_overlap(left: &[AlbumTrackEvidence], right: &[AlbumTrackEvidence]) -> usize {
    let mut used = vec![false; right.len()];
    let mut matched = 0;
    for candidate in left {
        let title = logical_title(&candidate.title);
        if title.is_empty() {
            continue;
        }
        if let Some((index, _)) = right.iter().enumerate().find(|(index, other)| {
            !used[*index]
                && logical_title(&other.title) == title
                && candidate.duration_secs.abs_diff(other.duration_secs) <= 5
        }) {
            used[index] = true;
            matched += 1;
        }
    }
    matched
}

fn logical_title(value: &str) -> String {
    normalize_artist(strip_edition_suffix(value))
}

pub(crate) fn edition_descriptor(value: &str) -> String {
    let trimmed = value.trim();
    let stripped = strip_edition_suffix(trimmed).trim();
    if stripped.len() == trimmed.len() {
        String::new()
    } else {
        trimmed[stripped.len()..]
            .trim_matches(|ch: char| ch.is_whitespace() || "-–—()[]{}".contains(ch))
            .trim()
            .to_string()
    }
}

fn strip_edition_suffix(value: &str) -> &str {
    const MARKERS: [&str; 15] = [
        "remaster",
        "deluxe",
        "expanded",
        "anniversary",
        "edition",
        "reissue",
        "re-release",
        "mix",
        "version",
        "mono",
        "stereo",
        "sacd",
        "bonus",
        "legacy",
        "collector",
    ];
    let marked = |tail: &str| MARKERS.iter().any(|marker| tail.contains(marker));
    if let Some(start) = value
        .char_indices()
        .rev()
        .find(|(index, ch)| {
            matches!(*ch, '(' | '[' | '{')
                && marked(&value[*index + ch.len_utf8()..].to_lowercase())
        })
        .map(|(index, _)| index)
    {
        return value[..start].trim_end();
    }
    for separator in [" - ", " – ", " — "] {
        if let Some(index) = value.rfind(separator) {
            if marked(&value[index + separator.len()..].to_lowercase()) {
                return value[..index].trim_end();
            }
        }
    }
    value.trim()
}

/// Folders tab, FLAT mode: the album grid grouped by DIRECTORY. Local-only
/// by definition — Plex rows have no filesystem folder.
pub fn load_folders_blocking() -> Result<Vec<AlbumRow>, String> {
    let exclude_network = crate::offline_fwd::exclude_network_folders_now();
    let albums = with_db(|db| {
        db.get_albums_with_full_filter(
            /* include_hidden */ false,
            /* include_qobuz_downloads */ true,
            exclude_network,
        )
    })
    .ok_or_else(|| "local library not available".to_string())?;
    let rows = with_art(|art| {
        albums
            .into_iter()
            .map(|a| map_album(a, art))
            .collect::<Vec<AlbumRow>>()
    });
    state(|s| {
        s.counts.folders = rows.len() as i64;
        s.folders = rows.clone();
    });
    Ok(rows)
}

/// Artists tab: the on-disk artists merged by NORMALIZED name, with the
/// aggregated Plex artists folded into the same buckets.
///
/// PARITY-DEBT #7 — this used to key the local<->Plex merge on a raw
/// `trim().to_lowercase()`, so "Beyoncé" and "Beyonce" (or "Sigur Rós" and
/// "Sigur Ros") were two rows with the user's albums and tracks split between
/// them. The key is `local_artist_match::normalize_artist` now (lowercase +
/// diacritic fold + punctuation collapse), which is also what lets the whole
/// reference merge come across: canonical spelling, summed track counts,
/// album counts recomputed from the album set (so a cross-credited album
/// counts for every contributor) and the custom -> Plex thumb -> album cover
/// portrait chain. Reference: `local_library.rs:3261-3358 merge_artists` and
/// its caller at :3540-3607.
pub fn load_artists_blocking() -> Result<Vec<ArtistRow>, String> {
    let plex_on = crate::local_plex::is_configured();
    let mode = group_mode();
    let plex_path = crate::local_plex::cache_db_path();
    // The SHARED remote mirror, and which of its sources may show. Both gates
    // matter: the path short-circuits the ATTACH for a user with no media
    // server, and the words are what make the master toggle actually remove a
    // server's rows from the grid (the mirror holds them all).
    let remote_path = crate::media_servers_qt::remote_cache_path();
    let remote_words = crate::media_servers_qt::configured_words();
    let remote_on = !remote_words.is_empty();
    let exclude_network = crate::offline_fwd::exclude_network_folders_now();

    // ONE db open for the three reads the merge needs. The album set is the
    // SAME query the Albums tab runs (Plex-aware union when the toggle is on),
    // so an artist's album count matches the grid the user sees.
    let (artists, albums, custom) = with_db(|db| {
        let artists =
            db.get_artists_with_filter(/* include_qobuz_downloads */ true, exclude_network)?;
        let albums = if plex_on || remote_on {
            db.get_albums_metadata_page(
                0,
                100_000,
                None,
                "artist",
                "asc",
                /* include_qobuz_downloads */ true,
                exclude_network,
                plex_path.as_deref(),
                remote_path.as_deref(),
                &remote_words,
                mode,
            )?
            .albums
        } else {
            db.get_albums_with_full_filter(
                /* include_hidden */ false,
                /* include_qobuz_downloads */ true,
                exclude_network,
            )?
        };
        // Custom AND previously-cached (Qobuz-fetched) portraits. A missing
        // `artist_images` table must not fail the whole tab.
        let custom = db.get_all_artist_image_urls().unwrap_or_default();
        Ok((artists, albums, custom))
    })
    .unwrap_or_default();

    let mut inputs: Vec<ArtistInput> = artists
        .into_iter()
        .map(|a| ArtistInput {
            name: a.name,
            album_count: a.album_count,
            track_count: a.track_count,
            source: "local",
        })
        .collect();

    // Plex artists join the SAME merge input (not a second pass keyed by a
    // different rule), so a local and a Plex "Radiohead" collapse to one row.
    let mut plex_portraits: HashMap<String, String> = HashMap::new();
    if plex_on {
        for pa in crate::local_plex::cached_artists() {
            let n = normalize_artist(&pa.name);
            if n.is_empty() {
                continue;
            }
            if let Some(path) = pa.artwork_path.clone().filter(|p| !p.is_empty()) {
                plex_portraits.entry(n).or_insert(path);
            }
            inputs.push(ArtistInput {
                name: pa.name,
                album_count: pa.album_count,
                track_count: pa.track_count,
                source: "plex",
            });
        }
    }

    for (source, remote) in crate::media_servers_qt::cached_artists() {
        inputs.push(ArtistInput {
            name: remote.name,
            album_count: remote.album_count,
            track_count: remote.track_count,
            source,
        });
    }

    let credits: Vec<AlbumCredit<'_>> = albums
        .iter()
        .map(|a| AlbumCredit {
            source: a.source.as_str(),
            id: a.id.as_str(),
            artist: a.artist.as_str(),
            all_artists: a.all_artists.as_str(),
            artwork_path: a.artwork_path.as_deref().unwrap_or(""),
        })
        .collect();

    let merged = merge_artists(
        inputs,
        &credits,
        &custom,
        &plex_portraits,
        plex_on || remote_on,
    );

    // One alias corpus for both the identity merge above and the facet pass.
    // Facets follow credited albums, never the portrait chosen for the row:
    // the latter is one image from one source and cannot describe a mixed
    // artist's actual source/format/quality/year availability.
    let mut family_names = albums
        .iter()
        .map(|album| album.artist.as_str())
        .collect::<Vec<_>>();
    for album in &albums {
        family_names.extend(album.all_artists.split(',').filter(|name| !name.is_empty()));
    }
    let aliases = build_artist_family_aliases(&family_names);

    // The artwork index is keyed on the DISPLAY name (`artist:{name}`), so the
    // portrait is registered under the CANONICAL spelling the row carries —
    // registering it under the Plex spelling is what left a merged row with a
    // key nobody had a source for. Overwrite rather than `or_insert`: this
    // pass has just recomputed the chain, and a stale entry from a previous
    // album-identity mode must not win.
    let rows: Vec<ArtistRow> = with_art(|art| {
        merged
            .into_iter()
            .map(|m| {
                let key = artist_key(&m.name);
                let cached = crate::local_artist_images_qt::register(
                    &key,
                    &m.name,
                    !m.image_path.is_empty(),
                );
                let image_path = cached.as_deref().unwrap_or(&m.image_path);
                let image_source = if cached.is_some() {
                    "local"
                } else {
                    m.image_source.as_str()
                };
                if !image_path.is_empty() {
                    if let Some(t) = crate::local_rows::art_token(Some(image_source), image_path) {
                        art.insert(key.clone(), t);
                    }
                }
                let selected = normalize_artist(&m.name);
                let mut sources = Vec::new();
                let mut formats = Vec::new();
                let mut quality_tiers = Vec::new();
                let mut years = Vec::new();
                for album in albums.iter().filter(|album| {
                    album_matches_artist_with_aliases(
                        &album.artist,
                        &album.all_artists,
                        &selected,
                        &aliases,
                    )
                }) {
                    let source_values = if album.sources.is_empty() {
                        std::slice::from_ref(&album.source)
                    } else {
                        album.sources.as_slice()
                    };
                    for source in source_values {
                        let source = badge_source(Some(source));
                        if !sources.iter().any(|value: &String| value == &source) {
                            sources.push(source);
                        }
                    }
                    let format = album.format.to_string().to_ascii_lowercase();
                    if !formats.iter().any(|value: &String| value == &format) {
                        formats.push(format);
                    }
                    let tier =
                        tier_of(&album.format, album.bit_depth, album.sample_rate).to_string();
                    if !tier.is_empty()
                        && !quality_tiers.iter().any(|value: &String| value == &tier)
                    {
                        quality_tiers.push(tier);
                    }
                    if let Some(year) = album.year {
                        if !years.contains(&year) {
                            years.push(year);
                        }
                    }
                }
                sources.sort();
                formats.sort();
                quality_tiers.sort();
                years.sort_unstable();
                ArtistRow {
                    art_key: key,
                    name: m.name,
                    album_count: m.album_count,
                    track_count: m.track_count,
                    source: m.source,
                    sources,
                    formats,
                    quality_tiers,
                    year: years.last().map(u32::to_string).unwrap_or_default(),
                    years,
                }
            })
            .collect()
    });

    state(|s| {
        s.counts.artists = rows.len() as i64;
        s.artists = rows.clone();
    });
    Ok(rows)
}

/// The ids of the CACHED album rows that credit `artist`, as a JSON array —
/// the Artists tab's right pane (PARITY-DEBT #8).
///
/// The match lives here, not in QML, because it is the same normalized-part
/// rule the merge above uses and there must be exactly one of it. The QML did
/// a lowercase equality on `artist` OR a SUBSTRING `indexOf` on `allArtists`,
/// which listed "Airbourne" and "Blair" under "Air" and hid an album credited
/// "A & B" from "B". Reference: `local_library.rs:3368-3390`.
///
/// Reads `state.albums` — the very document the QML renders — so the ids it
/// returns always resolve to rows the view already has.
pub fn artist_album_ids(artist: &str) -> String {
    let nsel = normalize_artist(artist);
    if nsel.is_empty() {
        return "[]".to_string();
    }
    let ids: Vec<String> = state(|s| {
        let mut names = s
            .albums
            .iter()
            .map(|album| album.artist.as_str())
            .collect::<Vec<_>>();
        for album in &s.albums {
            names.extend(album.all_artists.split(',').filter(|name| !name.is_empty()));
        }
        let aliases = build_artist_family_aliases(&names);
        s.albums
            .iter()
            .filter(|a| {
                album_matches_artist_with_aliases(&a.artist, &a.all_artists, &nsel, &aliases)
            })
            .map(|a| a.id.clone())
            .collect()
    });
    serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TrackSourceCounts {
    pub local: usize,
    pub plex: usize,
    pub jellyfin: usize,
    pub subsonic: usize,
}

pub struct TracksPageLoad {
    pub generation: u64,
    pub rows: Vec<TrackRow>,
    pub has_more: bool,
    pub page_rows: usize,
    pub candidates: TrackSourceCounts,
    pub published: TrackSourceCounts,
    pub query_time: std::time::Duration,
    pub merge_time: std::time::Duration,
    pub map_time: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TrackSourcePage {
    Local,
    Plex,
    Jellyfin,
    Subsonic,
}

impl TrackSourcePage {
    fn bump(self, offsets: &mut TrackSourceOffsets) {
        match self {
            Self::Local => offsets.local += 1,
            Self::Plex => offsets.plex += 1,
            Self::Jellyfin => offsets.jellyfin += 1,
            Self::Subsonic => offsets.subsonic += 1,
        }
    }

    fn bump_count(self, counts: &mut TrackSourceCounts) {
        match self {
            Self::Local => counts.local += 1,
            Self::Plex => counts.plex += 1,
            Self::Jellyfin => counts.jellyfin += 1,
            Self::Subsonic => counts.subsonic += 1,
        }
    }
}

struct CandidatePage {
    source: TrackSourcePage,
    rows: Vec<LocalTrack>,
}

struct MergedTrackPage {
    rows: Vec<LocalTrack>,
    consumed: TrackSourceOffsets,
    published: TrackSourceCounts,
    has_more: bool,
}

/// One globally ordered Tracks page. Each source query is bounded to one page
/// plus a look-ahead row; no source is materialized in full.
pub fn load_tracks_page_blocking(
    request: TracksLoadRequest,
) -> Result<Option<TracksPageLoad>, String> {
    let candidate_limit = TRACKS_PAGE + 1;
    let effective_sort = effective_tracks_sort(&request.sort, &request.group);
    let query_started = std::time::Instant::now();
    let exclude_network = crate::offline_fwd::exclude_network_folders_now();
    // No library.db is valid for a remote-only installation.
    let local =
        if request.filter.source_enabled("local") || request.filter.source_enabled("offline") {
            with_db(|db| {
                db.search_with_filter_page_faceted(
                    request.query.trim(),
                    request.offsets.local,
                    candidate_limit,
                    true,
                    exclude_network,
                    effective_sort,
                    &request.filter.formats,
                    request.filter.other_formats,
                    &request.filter.qualities,
                    &request.filter.sources,
                )
            })
            .unwrap_or_default()
        } else {
            Vec::new()
        };
    if tracks_generation() != request.generation {
        return Ok(None);
    }
    let plex = if crate::local_plex::is_configured() && request.filter.source_enabled("plex") {
        crate::local_plex::search_tracks_page(
            &request.query,
            request.offsets.plex,
            candidate_limit,
            effective_sort,
            &request.filter.formats,
            request.filter.other_formats,
            &request.filter.qualities,
        )
    } else {
        Vec::new()
    };
    if tracks_generation() != request.generation {
        return Ok(None);
    }
    let jellyfin = if request.filter.source_enabled("jellyfin") {
        crate::media_servers_qt::search_tracks_page(
            "jellyfin",
            &request.query,
            request.offsets.jellyfin,
            candidate_limit,
            effective_sort,
            &request.filter.formats,
            request.filter.other_formats,
            &request.filter.qualities,
        )
    } else {
        Vec::new()
    };
    if tracks_generation() != request.generation {
        return Ok(None);
    }
    let subsonic = if request.filter.source_enabled("subsonic") {
        crate::media_servers_qt::search_tracks_page(
            "subsonic",
            &request.query,
            request.offsets.subsonic,
            candidate_limit,
            effective_sort,
            &request.filter.formats,
            request.filter.other_formats,
            &request.filter.qualities,
        )
    } else {
        Vec::new()
    };
    let query_time = query_started.elapsed();
    let candidates = TrackSourceCounts {
        local: local.len(),
        plex: plex.len(),
        jellyfin: jellyfin.len(),
        subsonic: subsonic.len(),
    };
    if tracks_generation() != request.generation {
        return Ok(None);
    }

    let merge_started = std::time::Instant::now();
    let merged = merge_track_pages(
        vec![
            CandidatePage {
                source: TrackSourcePage::Local,
                rows: local,
            },
            CandidatePage {
                source: TrackSourcePage::Plex,
                rows: plex,
            },
            CandidatePage {
                source: TrackSourcePage::Jellyfin,
                rows: jellyfin,
            },
            CandidatePage {
                source: TrackSourcePage::Subsonic,
                rows: subsonic,
            },
        ],
        effective_sort,
        TRACKS_PAGE as usize,
    );
    let merge_time = merge_started.elapsed();
    if tracks_generation() != request.generation {
        return Ok(None);
    }

    let page_rows = merged.rows.len();
    let raw = merged.rows;
    let map_started = std::time::Instant::now();
    let mut page_art = HashMap::new();
    let rows = raw
        .iter()
        .map(|track| map_track(track, &mut page_art))
        .collect::<Vec<TrackRow>>();
    let map_time = map_started.elapsed();
    let Some(all) = commit_tracks_page(
        &request,
        rows,
        raw,
        page_art,
        merged.consumed,
        merged.has_more,
    ) else {
        return Ok(None);
    };

    Ok(Some(TracksPageLoad {
        generation: request.generation,
        rows: all,
        has_more: merged.has_more,
        page_rows,
        candidates,
        published: merged.published,
        query_time,
        merge_time,
        map_time,
    }))
}

/// Group headers require their key to be globally contiguous. Grouping thus
/// owns the query order (matching the native catalog descriptor) while the
/// toolbar sort remains persisted for when grouping is switched off.
fn effective_tracks_sort<'a>(sort: &'a str, group: &str) -> &'a str {
    match group {
        "album" => "default",
        // Unlike artist-asc, grouping is by the performing track artist, not
        // album_artist. This is also the key printed in the group header.
        "artist" => "group-artist",
        "name" => "title-asc",
        _ => sort,
    }
}

fn merge_track_pages(pages: Vec<CandidatePage>, sort: &str, limit: usize) -> MergedTrackPage {
    let mut candidates: Vec<(TrackSourcePage, LocalTrack)> = pages
        .into_iter()
        .flat_map(|page| page.rows.into_iter().map(move |row| (page.source, row)))
        .collect();
    candidates.sort_by(|(source_a, a), (source_b, b)| {
        compare_tracks(a, b, sort)
            .then(source_a.cmp(source_b))
            .then_with(|| match source_a {
                // Plex SQL ends in rating_key; its published numeric id can
                // be a hash and therefore is not an equivalent tie-breaker.
                TrackSourcePage::Plex => a.file_path.cmp(&b.file_path),
                _ => a.id.cmp(&b.id),
            })
            .then(a.file_path.cmp(&b.file_path))
    });
    let has_more = candidates.len() > limit;
    candidates.truncate(limit);

    let mut consumed = TrackSourceOffsets::default();
    let mut published = TrackSourceCounts::default();
    let mut rows = Vec::with_capacity(candidates.len());
    for (source, row) in candidates {
        source.bump(&mut consumed);
        source.bump_count(&mut published);
        rows.push(row);
    }
    MergedTrackPage {
        rows,
        consumed,
        published,
        has_more,
    }
}

/// The tab badges. Cheap: the Tracks count never materializes the table.
pub fn load_counts_blocking() -> LocalCounts {
    let local_tracks = with_db(|db| db.count_all_local_tracks()).unwrap_or(0) as i64;
    let plex_tracks = if crate::local_plex::is_configured() {
        crate::local_plex::cached_track_count()
    } else {
        0
    };
    let (jellyfin_tracks, subsonic_tracks) = crate::media_servers_qt::cached_track_counts();
    let total_tracks = local_tracks + plex_tracks + jellyfin_tracks + subsonic_tracks;
    log::info!(
        "[qbz-qt][library] source counts: local={local_tracks} plex={plex_tracks} jellyfin={jellyfin_tracks} subsonic={subsonic_tracks} total={total_tracks}"
    );
    state(|s| {
        s.counts.tracks = total_tracks;
        s.counts.plex_tracks = plex_tracks;
        s.counts.jellyfin_tracks = jellyfin_tracks;
        s.counts.subsonic_tracks = subsonic_tracks;
        s.counts.clone()
    })
}

/// An album's tracks by group key: the Plex cache for a legacy content hash
/// or native edition key, else the ACTIVE identity mode's query against
/// `library.db`.
pub fn fetch_album_tracks_blocking(id: &str) -> Vec<LocalTrack> {
    let version_ids = state(|s| s.album_version_ids.get(id).cloned());
    if let Some(version_ids) = version_ids {
        return version_ids
            .into_iter()
            .flat_map(|version_id| fetch_source_album_tracks_blocking(&version_id))
            .collect();
    }
    fetch_source_album_tracks_blocking(id)
}

fn fetch_source_album_tracks_blocking(id: &str) -> Vec<LocalTrack> {
    if id.starts_with("plex:") {
        return crate::local_plex::album_tracks(id);
    }
    // A media-server key (`jellyfin:<albumId>` / `subsonic:<albumId>`). WITHOUT
    // this arm the key fell through to `library.db`, where it matches nothing:
    // the album page opened EMPTY, and in metadata mode it paid two fruitless
    // full queries first — which is why leaving a Jellyfin album was slower
    // than leaving a local one.
    if let Some(rows) = crate::media_servers_qt::album_tracks(id) {
        return rows;
    }
    let mode = group_mode();
    with_db(|db| match mode {
        AlbumGroupMode::Metadata => {
            let meta = db.get_album_tracks_metadata(id)?;
            if meta.is_empty() {
                db.get_album_tracks(id)
            } else {
                Ok(meta)
            }
        }
        AlbumGroupMode::Folder => db.get_album_tracks(id),
    })
    .unwrap_or_default()
}

/// The local album-detail document (header + versions + track list). `id` is
/// the group key of the ACTIVE identity mode, or a Plex legacy/native key.
///
/// The tracks are split into VERSIONS by source directory before the header is
/// built — two physical copies of the same album must not merge into one
/// duplicated track list (album/LocalAlbumView.slint `open_local_album`). The
/// header, the picker entries and the raw-row cache are all owned by
/// `local_album_actions`, which is also what the version picker and the
/// per-disc menu read back.
/// The routed Local Library detail, constrained to the media funnel that was
/// active at the click site. A logical card may represent several sources;
/// opening it must not make filtered-out physical versions selectable again.
pub fn load_album_detail_filtered_blocking(id: &str, filter_json: &str) -> Option<AlbumDetailDoc> {
    let mut tracks = fetch_album_tracks_blocking(id);
    if tracks.is_empty() {
        return None;
    }
    // Backfill covers from cover.jpg/folder.jpg on disk (the DB may not have an
    // artwork_path even when a cover sits in the folder) — the reference does
    // exactly this here, `local_library.rs:1826`. It is what makes the detail
    // rows AND everything that later reads `detail_raw` (the per-disc menu, the
    // bulk bar, `find_track_blocking`) carry the cover the folder has.
    crate::local_playback::fill_missing_covers(&mut tracks);
    let filter = crate::local_filter::MediaFilter::from_json(filter_json);
    tracks.retain(|track| filter.track_enabled(track));
    crate::local_album_actions::open_versions(id, tracks)
}

/// Comparator shared by the bounded candidate merge and its deterministic
/// tests. It mirrors every source query's allowlisted ORDER BY. Source rank
/// and the source-native identity are appended by `merge_track_pages`.
fn compare_tracks(a: &LocalTrack, b: &LocalTrack, sort: &str) -> std::cmp::Ordering {
    // SQLite NOCASE folds ASCII only. Unicode lowercasing would disagree with
    // SQLite at a page boundary and could skip a row.
    let lc = |s: &str| s.to_ascii_lowercase();
    let artist_key = |t: &LocalTrack| lc(t.album_artist.as_deref().unwrap_or(&t.artist));
    let album_tail = |a: &LocalTrack, b: &LocalTrack| {
        lc(&a.album)
            .cmp(&lc(&b.album))
            .then(a.disc_number.cmp(&b.disc_number))
            .then(a.track_number.cmp(&b.track_number))
    };
    let year_cmp = |a: &LocalTrack, b: &LocalTrack, desc: bool| match (a.year, b.year) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(ya), Some(yb)) => {
            if desc {
                yb.cmp(&ya)
            } else {
                ya.cmp(&yb)
            }
        }
    };
    match sort {
        "title-asc" => lc(&a.title)
            .cmp(&lc(&b.title))
            .then(lc(&a.artist).cmp(&lc(&b.artist))),
        "title-desc" => lc(&b.title)
            .cmp(&lc(&a.title))
            .then(lc(&a.artist).cmp(&lc(&b.artist))),
        "artist-asc" => artist_key(a).cmp(&artist_key(b)).then(album_tail(a, b)),
        "artist-desc" => artist_key(b).cmp(&artist_key(a)).then(album_tail(a, b)),
        "group-artist" => lc(&a.artist)
            .cmp(&lc(&b.artist))
            .then(lc(&a.album).cmp(&lc(&b.album)))
            .then(lc(&a.title).cmp(&lc(&b.title))),
        "year-desc" => year_cmp(a, b, true).then(album_tail(a, b)),
        "year-asc" => year_cmp(a, b, false).then(album_tail(a, b)),
        "added-desc" => b.indexed_at.cmp(&a.indexed_at).then(album_tail(a, b)),
        _ => lc(&a.album)
            .cmp(&lc(&b.album))
            .then(artist_key(a).cmp(&artist_key(b)))
            .then(a.disc_number.cmp(&b.disc_number))
            .then(a.track_number.cmp(&b.track_number))
            .then(lc(&a.title).cmp(&lc(&b.title))),
    }
}

#[cfg(test)]
mod phase_a_tests {
    use std::collections::{HashMap, HashSet};

    use super::*;

    struct Fixture {
        local: Vec<LocalTrack>,
        plex: Vec<LocalTrack>,
        jellyfin: Vec<LocalTrack>,
        subsonic: Vec<LocalTrack>,
    }

    fn source_rank(source: TrackSourcePage) -> u64 {
        match source {
            TrackSourcePage::Local => 0,
            TrackSourcePage::Plex => 1,
            TrackSourcePage::Jellyfin => 2,
            TrackSourcePage::Subsonic => 3,
        }
    }

    fn source_of(row: &LocalTrack) -> TrackSourcePage {
        match row.source.as_deref() {
            Some("plex") => TrackSourcePage::Plex,
            Some("jellyfin") => TrackSourcePage::Jellyfin,
            Some("subsonic") => TrackSourcePage::Subsonic,
            _ => TrackSourcePage::Local,
        }
    }

    fn native_tie(source: TrackSourcePage, a: &LocalTrack, b: &LocalTrack) -> std::cmp::Ordering {
        match source {
            TrackSourcePage::Plex => a.file_path.cmp(&b.file_path),
            _ => a.id.cmp(&b.id),
        }
        .then(a.file_path.cmp(&b.file_path))
    }

    fn global_cmp(a: &LocalTrack, b: &LocalTrack, sort: &str) -> std::cmp::Ordering {
        let source_a = source_of(a);
        let source_b = source_of(b);
        compare_tracks(a, b, sort)
            .then(source_a.cmp(&source_b))
            .then_with(|| native_tie(source_a, a, b))
    }

    fn source_rows(source: TrackSourcePage, count: usize, sort: &str) -> Vec<LocalTrack> {
        let rank = source_rank(source) as usize;
        let mut rows = (0..count)
            .map(|i| {
                let global = i * 4 + rank;
                let (id, file_path, source_word, album_artist) = match source {
                    TrackSourcePage::Local => (
                        i as i64 + 1,
                        format!("/fixture/local-{i:05}.flac"),
                        None,
                        Some(format!("Artist {:02}", global % 17)),
                    ),
                    TrackSourcePage::Plex => (
                        (crate::local_plex::PLEX_TRACK_ID_FLOOR | (i as u64 + 1)) as i64,
                        format!("plex-{i:05}"),
                        Some("plex".to_string()),
                        None,
                    ),
                    TrackSourcePage::Jellyfin => (
                        qbz_media_cache::RemoteSource::Jellyfin.namespace(i as i64 + 1),
                        format!("jellyfin-{i:05}"),
                        Some("jellyfin".to_string()),
                        Some(format!("Artist {:02}", global % 17)),
                    ),
                    TrackSourcePage::Subsonic => (
                        qbz_media_cache::RemoteSource::Subsonic.namespace(i as i64 + 1),
                        format!("subsonic-{i:05}"),
                        Some("subsonic".to_string()),
                        Some(format!("Artist {:02}", global % 17)),
                    ),
                };
                LocalTrack {
                    id,
                    file_path,
                    title: format!("Track {:05}", global / 20),
                    artist: format!("Artist {:02}", global % 17),
                    album: format!("Album {:04}", global / 40),
                    album_artist,
                    album_group_key: format!("album-{}-{}", rank, global / 40),
                    album_group_title: format!("Album {:04}", global / 40),
                    track_number: Some((global % 10 + 1) as u32),
                    disc_number: Some(1),
                    year: Some(1980 + (global % 47) as u32),
                    indexed_at: global as i64,
                    source: source_word,
                    ..Default::default()
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| compare_tracks(a, b, sort).then_with(|| native_tie(source, a, b)));
        rows
    }

    fn fixture(local: usize, plex: usize, jellyfin: usize, subsonic: usize, sort: &str) -> Fixture {
        Fixture {
            local: source_rows(TrackSourcePage::Local, local, sort),
            plex: source_rows(TrackSourcePage::Plex, plex, sort),
            jellyfin: source_rows(TrackSourcePage::Jellyfin, jellyfin, sort),
            subsonic: source_rows(TrackSourcePage::Subsonic, subsonic, sort),
        }
    }

    fn candidate(rows: &[LocalTrack], offset: u64) -> Vec<LocalTrack> {
        rows.iter()
            .skip(offset as usize)
            .take(TRACKS_PAGE as usize + 1)
            .cloned()
            .collect()
    }

    fn page(fixture: &Fixture, offsets: TrackSourceOffsets, sort: &str) -> MergedTrackPage {
        merge_track_pages(
            vec![
                CandidatePage {
                    source: TrackSourcePage::Local,
                    rows: candidate(&fixture.local, offsets.local),
                },
                CandidatePage {
                    source: TrackSourcePage::Plex,
                    rows: candidate(&fixture.plex, offsets.plex),
                },
                CandidatePage {
                    source: TrackSourcePage::Jellyfin,
                    rows: candidate(&fixture.jellyfin, offsets.jellyfin),
                },
                CandidatePage {
                    source: TrackSourcePage::Subsonic,
                    rows: candidate(&fixture.subsonic, offsets.subsonic),
                },
            ],
            sort,
            TRACKS_PAGE as usize,
        )
    }

    fn collect_union(fixture: &Fixture, sort: &str) -> Vec<LocalTrack> {
        let mut offsets = TrackSourceOffsets::default();
        let mut out = Vec::new();
        loop {
            let merged = page(fixture, offsets, sort);
            offsets.add_assign(merged.consumed);
            let done = !merged.has_more;
            out.extend(merged.rows);
            if done {
                break;
            }
        }
        out
    }

    #[test]
    fn mixed_pages_are_globally_ordered_and_every_id_occurs_once() {
        let sort = "title-asc";
        let input = fixture(624, 5_137, 701, 650, sort);
        let rows = collect_union(&input, sort);
        assert_eq!(rows.len(), 7_112);
        let ids: HashSet<i64> = rows.iter().map(|row| row.id).collect();
        assert_eq!(ids.len(), rows.len());
        assert!(rows
            .windows(2)
            .all(|pair| global_cmp(&pair[0], &pair[1], sort).is_le()));
    }

    #[test]
    fn every_supported_sort_stays_global_across_page_boundaries() {
        for sort in [
            "default",
            "title-asc",
            "title-desc",
            "artist-asc",
            "artist-desc",
            "group-artist",
            "year-asc",
            "year-desc",
            "added-desc",
        ] {
            let input = fixture(377, 513, 421, 389, sort);
            let rows = collect_union(&input, sort);
            assert_eq!(rows.len(), 1_700, "sort {sort}");
            let ids: HashSet<i64> = rows.iter().map(|row| row.id).collect();
            assert_eq!(ids.len(), rows.len(), "sort {sort}");
            assert!(
                rows.windows(2)
                    .all(|pair| global_cmp(&pair[0], &pair[1], sort).is_le()),
                "sort {sort}"
            );
        }
    }

    #[test]
    fn grouped_pages_keep_the_first_page_as_an_immutable_prefix() {
        let effective = effective_tracks_sort("added-desc", "artist");
        assert_eq!(effective, "group-artist");
        let input = fixture(624, 1_337, 701, 650, effective);
        let first = page(&input, TrackSourceOffsets::default(), effective);
        assert_eq!(first.rows.len(), TRACKS_PAGE as usize);
        assert!(first.has_more);

        let all = collect_union(&input, effective);
        let first_ids = first.rows.iter().map(|row| row.id).collect::<Vec<_>>();
        let all_prefix = all
            .iter()
            .take(first_ids.len())
            .map(|row| row.id)
            .collect::<Vec<_>>();
        assert_eq!(all_prefix, first_ids);
        assert!(all
            .windows(2)
            .all(|pair| global_cmp(&pair[0], &pair[1], effective).is_le()));

        assert_eq!(effective_tracks_sort("year-desc", "album"), "default");
        assert_eq!(effective_tracks_sort("year-desc", "name"), "title-asc");
        assert_eq!(effective_tracks_sort("year-desc", "off"), "year-desc");
    }

    #[test]
    fn local_rows_remaining_after_a_mixed_first_page_are_not_skipped() {
        let sort = "title-asc";
        let input = fixture(624, 0, 800, 800, sort);
        let first = page(&input, TrackSourceOffsets::default(), sort);
        assert_eq!(first.rows.len(), TRACKS_PAGE as usize);
        assert!(first.consumed.local > 0);
        assert!(first.consumed.local < 624);
        assert_eq!(
            first.consumed.local as usize,
            first
                .rows
                .iter()
                .filter(|row| source_of(row) == TrackSourcePage::Local)
                .count()
        );

        let all = collect_union(&input, sort);
        assert_eq!(all.len(), 2_224);
        assert_eq!(
            all.iter()
                .filter(|row| source_of(row) == TrackSourcePage::Local)
                .count(),
            624
        );
    }

    #[test]
    fn remote_only_union_is_paged_and_complete() {
        let sort = "title-asc";
        let input = fixture(0, 0, 1_201, 0, sort);
        let first = page(&input, TrackSourceOffsets::default(), sort);
        assert_eq!(first.rows.len(), TRACKS_PAGE as usize);
        assert!(first.has_more);
        assert_eq!(first.consumed.local, 0);
        assert_eq!(collect_union(&input, sort).len(), 1_201);
    }

    #[test]
    fn local_only_and_plex_only_unions_are_complete() {
        let sort = "title-asc";
        let local = fixture(1_201, 0, 0, 0, sort);
        let plex = fixture(0, 5_137, 0, 0, sort);
        assert_eq!(collect_union(&local, sort).len(), 1_201);
        assert_eq!(collect_union(&plex, sort).len(), 5_137);
    }

    #[test]
    fn phase_a_first_page_metrics_before_and_after() {
        let sort = "title-asc";
        let input = fixture(624, 17_145, 4_924, 6_678, sort);

        let before_query_started = std::time::Instant::now();
        let mut before = input.plex.iter().take(5_000).cloned().collect::<Vec<_>>();
        before.extend(input.jellyfin.iter().cloned());
        before.extend(input.subsonic.iter().cloned());
        before.extend(input.local.iter().take(TRACKS_PAGE as usize).cloned());
        let before_query = before_query_started.elapsed();
        let before_merge_started = std::time::Instant::now();
        before.sort_by(|a, b| global_cmp(a, b, sort));
        let before_merge = before_merge_started.elapsed();
        let before_map_started = std::time::Instant::now();
        let mut before_art = HashMap::new();
        let before_rows = before
            .iter()
            .map(|row| map_track(row, &mut before_art))
            .collect::<Vec<_>>();
        let before_map = before_map_started.elapsed();
        let before_serialize_started = std::time::Instant::now();
        let before_json = serde_json::to_vec(&before_rows).unwrap();
        let before_serialize = before_serialize_started.elapsed();

        let after_query_started = std::time::Instant::now();
        let candidates = vec![
            CandidatePage {
                source: TrackSourcePage::Local,
                rows: candidate(&input.local, 0),
            },
            CandidatePage {
                source: TrackSourcePage::Plex,
                rows: candidate(&input.plex, 0),
            },
            CandidatePage {
                source: TrackSourcePage::Jellyfin,
                rows: candidate(&input.jellyfin, 0),
            },
            CandidatePage {
                source: TrackSourcePage::Subsonic,
                rows: candidate(&input.subsonic, 0),
            },
        ];
        let after_query = after_query_started.elapsed();
        let after_merge_started = std::time::Instant::now();
        let after = merge_track_pages(candidates, sort, TRACKS_PAGE as usize);
        let after_merge = after_merge_started.elapsed();
        let after_map_started = std::time::Instant::now();
        let mut after_art = HashMap::new();
        let after_rows = after
            .rows
            .iter()
            .map(|row| map_track(row, &mut after_art))
            .collect::<Vec<_>>();
        let after_map = after_map_started.elapsed();
        let after_serialize_started = std::time::Instant::now();
        let after_json = serde_json::to_vec(&after_rows).unwrap();
        let after_serialize = after_serialize_started.elapsed();

        assert_eq!(before_rows.len(), 17_102);
        assert_eq!(after_rows.len(), TRACKS_PAGE as usize);
        assert!(after.has_more);
        assert!(after_json.len() < before_json.len());
        println!(
            "[phase-a-metrics] broad='Track' counts local=624 plex=17145 jellyfin=4924 subsonic=6678 total=29371; before rows={} json_bytes={} query_us={} merge_us={} map_us={} serialize_us={}; after rows={} json_bytes={} query_us={} merge_us={} map_us={} serialize_us={} selected local={} plex={} jellyfin={} subsonic={}",
            before_rows.len(),
            before_json.len(),
            before_query.as_micros(),
            before_merge.as_micros(),
            before_map.as_micros(),
            before_serialize.as_micros(),
            after_rows.len(),
            after_json.len(),
            after_query.as_micros(),
            after_merge.as_micros(),
            after_map.as_micros(),
            after_serialize.as_micros(),
            after.published.local,
            after.published.plex,
            after.published.jellyfin,
            after.published.subsonic,
        );
    }

    fn album_copy(
        id: &str,
        source: &str,
        title: &str,
        tracks: &[(&str, u64)],
        bit_depth: u32,
        sample_rate: f64,
    ) -> LocalAlbum {
        LocalAlbum {
            id: id.to_string(),
            title: title.to_string(),
            artist: "Talk Talk".to_string(),
            all_artists: "Talk Talk".to_string(),
            year: Some(1988),
            catalog_number: None,
            genres: Vec::new(),
            artwork_path: None,
            artwork_source: None,
            track_count: tracks.len() as u32,
            total_duration_secs: tracks.iter().map(|(_, duration)| duration).sum(),
            format: AudioFormat::Flac,
            bit_depth: Some(bit_depth),
            sample_rate,
            directory_path: String::new(),
            source_folders: None,
            source: source.to_string(),
            sources: vec![source.to_string()],
            identity_tracks: tracks
                .iter()
                .map(|(title, duration_secs)| AlbumTrackEvidence {
                    title: (*title).to_string(),
                    duration_secs: *duration_secs,
                })
                .collect(),
        }
    }

    #[test]
    fn album_copies_require_content_evidence_and_keep_every_source() {
        let tracks = [
            ("The Rainbow", 568),
            ("Eden", 421),
            ("Desire", 439),
            ("Inheritance", 341),
        ];
        let local = album_copy(
            "eden|talk talk",
            "user",
            "Spirit of Eden",
            &tracks,
            16,
            44_100.0,
        );
        let plex = album_copy(
            "plex:eden",
            "plex",
            "Spirit of Eden (2012 Remaster)",
            &tracks,
            24,
            96_000.0,
        );
        let (rows, ids) = coalesce_album_versions(vec![local, plex]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "plex");
        assert_eq!(rows[0].sources, vec!["user", "plex"]);
        assert_eq!(ids.get(&rows[0].id).map(Vec::len), Some(2));

        let unrelated = album_copy(
            "subsonic:other",
            "subsonic",
            "Spirit of Eden",
            &[("Completely Different", 568), ("Still Different", 421)],
            24,
            96_000.0,
        );
        let (rows, _) = coalesce_album_versions(vec![rows[0].clone(), unrelated]);
        assert_eq!(rows.len(), 2, "title and artist alone must never merge");
    }

    #[test]
    fn logical_album_keeps_all_media_variants_and_selects_dsd_by_default() {
        let tracks = [
            ("Holy Wars", 383),
            ("Hangar 18", 311),
            ("Take No Prisoners", 208),
        ];
        let pcm = album_copy("pcm-24-96", "user", "Rust in Peace", &tracks, 24, 96_000.0);
        let mut dsd = album_copy(
            "purchased-dsd",
            "qobuz_purchase",
            "Rust in Peace",
            &tracks,
            1,
            2_822_400.0,
        );
        dsd.format = AudioFormat::Dsd;
        let physical = [&pcm, &dsd]
            .into_iter()
            .map(|album| (album.id.clone(), album_media_variants(album)))
            .collect::<HashMap<_, _>>();

        let (rows, version_ids) = coalesce_album_versions(vec![pcm, dsd]);
        assert_eq!(rows.len(), 1);
        let logical = &rows[0];
        assert_eq!(logical.format, AudioFormat::Dsd);
        assert_eq!(
            tier_of(&logical.format, logical.bit_depth, logical.sample_rate),
            "dsd"
        );

        let variants = logical_album_media_variants(
            logical,
            version_ids.get(&logical.id).map(Vec::as_slice),
            &physical,
        );
        assert!(variants.iter().any(|variant| {
            variant.quality_tier == "dsd"
                && variant.format == "DSD"
                && variant.source == "qobuz_purchase"
        }));
        assert!(variants.iter().any(|variant| {
            variant.quality_tier == "hires" && variant.format == "FLAC" && variant.source == "local"
        }));
    }

    #[test]
    fn coverless_best_audio_copy_uses_the_best_available_group_cover() {
        let tracks = [("Eres Toda una Mujer", 209), ("Amar y Querer", 191)];
        let mut jellyfin = album_copy(
            "jellyfin:romanticos",
            "jellyfin",
            "Siempre Romanticos!",
            &tracks,
            24,
            96_000.0,
        );
        jellyfin.artwork_path = None;
        let mut plex = album_copy(
            "plex:romanticos",
            "plex",
            "Siempre Romanticos!",
            &tracks,
            16,
            44_100.0,
        );
        plex.artwork_path = Some("/library/metadata/romanticos/thumb".to_string());

        let (rows, _) = coalesce_album_versions(vec![plex, jellyfin]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "jellyfin");
        assert_eq!(
            rows[0].artwork_path.as_deref(),
            Some("/library/metadata/romanticos/thumb")
        );
        assert_eq!(rows[0].artwork_source.as_deref(), Some("plex"));
    }

    #[test]
    fn album_copy_matching_allows_bonus_tracks_but_not_one_shared_compilation_track() {
        let base = [
            ("One", 201),
            ("Two", 202),
            ("Three", 203),
            ("Four", 204),
            ("Five", 205),
        ];
        let mut deluxe = base.to_vec();
        deluxe.push(("Bonus A", 180));
        deluxe.push(("Bonus B", 181));
        assert!(copies_match(
            &album_copy("a", "user", "Album", &base, 16, 44_100.0),
            &album_copy(
                "b",
                "jellyfin",
                "Album (Deluxe Edition)",
                &deluxe,
                16,
                44_100.0
            )
        ));
        assert!(!copies_match(
            &album_copy("a", "user", "Album", &base, 16, 44_100.0),
            &album_copy(
                "c",
                "subsonic",
                "Album",
                &[("One", 201), ("Else", 202), ("Other", 203), ("Nope", 204)],
                16,
                44_100.0,
            )
        ));
    }

    #[test]
    fn edition_suffix_inference_is_deliberately_narrow() {
        assert_eq!(
            logical_title("Spirit of Eden (2012 Remaster)"),
            "spirit of eden"
        );
        assert_eq!(
            edition_descriptor("Spirit of Eden (2012 Remaster)"),
            "2012 Remaster"
        );
        assert_eq!(
            edition_descriptor("Album - Deluxe Edition"),
            "Deluxe Edition"
        );
        assert_eq!(edition_descriptor("Album (Live at Montreux)"), "");
    }
}
