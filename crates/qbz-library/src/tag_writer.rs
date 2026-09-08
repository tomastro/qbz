//! Direct embedded-tag writer (frontend-agnostic port of the Tauri
//! `v2_library_write_album_metadata_to_files` lofty loop). The Slint and Tauri
//! frontends both call this so the lofty logic lives in one place. Progress is
//! reported through an `on_progress` closure (no Tauri event bus); the caller
//! orchestrates the DB update + sidecar removal.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::OpenOptions;
use std::path::Path;

use crate::{LibraryError, LocalTrack};

/// ID3 generation used when the container's canonical tag is ID3v2.
///
/// v2.4 is the standards-current default. v2.3 is an explicit compatibility
/// option for older hardware and software; it is never selected merely
/// because a file happened to arrive with a legacy tag.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Id3v2WriteVersion {
    #[default]
    V24,
    V23,
}

/// Direct-write policy. The default updates only the container's canonical
/// tag and preserves every secondary tag byte-for-byte as far as Lofty permits.
/// `synchronize_secondary_tags` is opt-in because a secondary tag may be there
/// specifically for an old player with stricter text/length limits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectTagWriteOptions {
    pub id3v2_version: Id3v2WriteVersion,
    pub synchronize_secondary_tags: bool,
}

/// One tag layer observed across the selected physical album version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLayerInspection {
    pub name: String,
    pub file_count: usize,
    pub canonical_file_count: usize,
    pub writable_file_count: usize,
}

/// Bounded preflight information for the editor. No audio or artwork bytes
/// escape this API; only format/tag names and aggregate counts do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlbumTagInspection {
    pub file_count: usize,
    pub canonical_layers: Vec<TagLayerInspection>,
    pub present_layers: Vec<TagLayerInspection>,
    /// Files where two non-empty tag layers disagree on at least one field the
    /// editor owns. The canonical layer still wins deterministically.
    pub conflicting_files: usize,
    /// Files whose canonical tag can be written by this Lofty build.
    pub writable_files: usize,
}

impl AlbumTagInspection {
    pub fn direct_write_supported(&self) -> bool {
        self.file_count > 0 && self.file_count == self.writable_files
    }
}

fn tag_type_name(tag_type: lofty::tag::TagType) -> &'static str {
    use lofty::tag::TagType;
    match tag_type {
        TagType::Ape => "APEv2",
        TagType::Id3v1 => "ID3v1",
        TagType::Id3v2 => "ID3v2",
        TagType::Mp4Ilst => "MP4 ilst",
        TagType::VorbisComments => "Vorbis comments",
        TagType::RiffInfo => "RIFF INFO",
        TagType::AiffText => "AIFF text",
        _ => "Other",
    }
}

fn tag_has_editor_conflict(tagged_file: &lofty::file::TaggedFile) -> bool {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;

    fn distinct_text(tagged_file: &lofty::file::TaggedFile, key: ItemKey) -> bool {
        let mut values = Vec::<String>::new();
        for tag in tagged_file.tags() {
            if let Some(value) = tag
                .get_string(key.clone())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if !values.iter().any(|seen| seen.eq_ignore_ascii_case(value)) {
                    values.push(value.to_string());
                }
            }
        }
        values.len() > 1
    }

    let text_conflict = [
        ItemKey::TrackTitle,
        ItemKey::TrackArtist,
        ItemKey::AlbumTitle,
        ItemKey::AlbumArtist,
        ItemKey::Genre,
        ItemKey::RecordingDate,
        ItemKey::Year,
        ItemKey::CatalogNumber,
    ]
    .into_iter()
    .any(|key| distinct_text(tagged_file, key));
    if text_conflict {
        return true;
    }

    let mut tracks = HashSet::new();
    let mut discs = HashSet::new();
    for tag in tagged_file.tags() {
        if let Some(value) = tag.track() {
            tracks.insert(value);
        }
        if let Some(value) = tag.disk() {
            discs.insert(value);
        }
    }
    tracks.len() > 1 || discs.len() > 1
}

/// Inspect every distinct file before the editor offers direct write.
///
/// All files are opened read-only. A malformed/unsupported member fails the
/// inspection rather than silently producing a reassuring partial summary.
pub fn inspect_album_tag_layers(paths: &[String]) -> Result<AlbumTagInspection, LibraryError> {
    use lofty::prelude::*;

    #[derive(Default)]
    struct Counts {
        present: usize,
        canonical: usize,
        writable: usize,
    }

    let mut seen = HashSet::new();
    let unique: Vec<&String> = paths
        .iter()
        .filter(|path| seen.insert((*path).clone()))
        .collect();
    if unique.is_empty() {
        return Err(LibraryError::Metadata(
            "No audio files were provided for tag inspection.".to_string(),
        ));
    }

    let mut layers = BTreeMap::<String, Counts>::new();
    let mut canonical = BTreeMap::<String, Counts>::new();
    let mut conflicting_files = 0usize;
    let mut writable_files = 0usize;

    for path in &unique {
        let file_path = Path::new(path);
        if !file_path.is_file() {
            return Err(LibraryError::Metadata(format!(
                "Audio file not found: {}",
                file_path.display()
            )));
        }
        let tagged_file = lofty::read_from_path(file_path).map_err(|error| {
            LibraryError::Metadata(format!(
                "Failed to inspect tags in {}: {error}",
                file_path.display()
            ))
        })?;
        let primary = tagged_file.primary_tag_type();
        let primary_name = tag_type_name(primary).to_string();
        let support = tagged_file.tag_support(primary);
        let counts = canonical.entry(primary_name).or_default();
        counts.canonical += 1;
        counts.present += usize::from(tagged_file.primary_tag().is_some());
        counts.writable += usize::from(support.is_writable());
        writable_files += usize::from(support.is_writable());

        for tag in tagged_file.tags() {
            let counts = layers
                .entry(tag_type_name(tag.tag_type()).to_string())
                .or_default();
            counts.present += 1;
            counts.canonical += usize::from(tag.tag_type() == primary);
            counts.writable += usize::from(tagged_file.tag_support(tag.tag_type()).is_writable());
        }
        conflicting_files += usize::from(tag_has_editor_conflict(&tagged_file));
    }

    let map_layers = |map: BTreeMap<String, Counts>| {
        map.into_iter()
            .map(|(name, counts)| TagLayerInspection {
                name,
                file_count: counts.present,
                canonical_file_count: counts.canonical,
                writable_file_count: counts.writable,
            })
            .collect()
    };

    Ok(AlbumTagInspection {
        file_count: unique.len(),
        canonical_layers: map_layers(canonical),
        present_layers: map_layers(layers),
        conflicting_files,
        writable_files,
    })
}

/// Album-level fields written into every file's embedded tags. A `None`
/// (or blank) field REMOVES that tag (direct write is destructive, unlike the
/// override-only sidecar).
#[derive(Debug, Clone)]
pub struct AlbumTagWrite {
    pub album_title: String,
    pub album_artist: String, // "" => remove the AlbumArtist tag
    pub year: Option<u32>,    // None => remove the date
    pub genre: Option<String>,
    pub catalog_number: Option<String>,
}

/// One file's per-track fields.
#[derive(Debug, Clone)]
pub struct TrackTagWrite {
    pub file_path: String,
    pub title: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}

/// Standards-oriented album fields owned by the expanded Qt editor.
///
/// `album_artist` in [`AlbumTagWrite`] remains the human-readable credit.
/// `album_artists` stores the ordered, lossless components in the dedicated
/// ALBUMARTISTS key where the target tag format supports it.
#[derive(Debug, Clone, Default)]
pub struct ExtendedAlbumTagWrite {
    pub album_artists: Vec<String>,
    pub compilation: Option<bool>,
    pub musicbrainz_release_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    pub musicbrainz_album_artist_ids: Vec<String>,
    /// Provenance retained by the sidecar. There is no interoperable Discogs
    /// key in Lofty's generic tag map, so it is deliberately not embedded.
    pub discogs_release_id: Option<String>,
}

/// Standards-oriented fields for one physical track.
#[derive(Debug, Clone, Default)]
pub struct ExtendedTrackTagWrite {
    pub file_path: String,
    /// Display credit, including intentional join phrases such as "A feat. B".
    pub artist_credit: String,
    /// Ordered components written to ARTISTS independently from ARTIST.
    pub artists: Vec<String>,
    pub composers: Vec<String>,
    pub performers: Vec<String>,
    pub musicbrainz_recording_id: Option<String>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_artist_ids: Vec<String>,
}

/// A validated front-cover candidate. Callers stage bytes before save; the
/// writer never follows a QML-provided path or URL while mutating audio files.
#[derive(Debug, Clone)]
pub struct FrontCoverWrite {
    pub bytes: Vec<u8>,
}

/// Decode and atomically install a conventional folder front cover.
/// The old file is restored if the final rename fails.
pub fn write_folder_front_cover(album_dir: &Path, bytes: &[u8]) -> Result<String, LibraryError> {
    if bytes.is_empty() || bytes.len() > 25 * 1024 * 1024 {
        return Err(LibraryError::Metadata(
            "The selected cover is empty or larger than 25 MiB.".to_string(),
        ));
    }
    let image = image::load_from_memory(bytes).map_err(|error| {
        LibraryError::Metadata(format!("The selected cover could not be decoded: {error}"))
    })?;
    if image.width() == 0
        || image.height() == 0
        || image.width() > 12_000
        || image.height() > 12_000
    {
        return Err(LibraryError::Metadata(
            "The selected cover has unsupported dimensions.".to_string(),
        ));
    }
    let target = album_dir.join("cover.jpg");
    let temporary = album_dir.join(".cover.jpg.qbz-tmp");
    let backup = album_dir.join(".cover.jpg.qbz-backup");
    image
        .save_with_format(&temporary, image::ImageFormat::Jpeg)
        .map_err(|error| LibraryError::Metadata(format!("Cover encode failed: {error}")))?;

    let had_target = target.is_file();
    if had_target {
        if backup.exists() {
            std::fs::remove_file(&backup).map_err(LibraryError::Io)?;
        }
        std::fs::rename(&target, &backup).map_err(LibraryError::Io)?;
    }
    if let Err(error) = std::fs::rename(&temporary, &target) {
        if had_target {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(LibraryError::Io(error));
    }
    if had_target && backup.exists() {
        std::fs::remove_file(&backup).map_err(LibraryError::Io)?;
    }
    Ok(target.to_string_lossy().to_string())
}

/// Rich values read from one file's canonical tag for editor seeding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditorTrackTagSnapshot {
    pub file_path: String,
    pub artist_credit: String,
    pub artists: Vec<String>,
    pub album_artists: Vec<String>,
    pub composers: Vec<String>,
    pub performers: Vec<String>,
    pub compilation: Option<bool>,
    pub musicbrainz_release_id: Option<String>,
    pub musicbrainz_release_group_id: Option<String>,
    pub musicbrainz_recording_id: Option<String>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_artist_ids: Vec<String>,
    pub musicbrainz_album_artist_ids: Vec<String>,
}

fn normalized_values(tag: &lofty::tag::Tag, key: lofty::tag::ItemKey) -> Vec<String> {
    let mut seen = HashSet::<String>::new();
    tag.get_strings(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(|value| {
            let folded = value.to_lowercase();
            seen.insert(folded).then(|| value.to_string())
        })
        .collect()
}

fn normalized_optional(tag: &lofty::tag::Tag, key: lofty::tag::ItemKey) -> Option<String> {
    normalized_values(tag, key).into_iter().next()
}

/// Read the canonical rich-tag projection used by the Qt editor.
/// Unreadable rows return an empty snapshot rather than borrowing values from
/// a neighbouring file; the ordinary editor preflight reports writability.
pub fn read_editor_tag_snapshots(paths: &[String]) -> Vec<EditorTrackTagSnapshot> {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;

    paths
        .iter()
        .map(|path| {
            let mut snapshot = EditorTrackTagSnapshot {
                file_path: path.clone(),
                ..EditorTrackTagSnapshot::default()
            };
            let Ok(file) = lofty::read_from_path(Path::new(path)) else {
                return snapshot;
            };
            let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
                return snapshot;
            };
            snapshot.artist_credit =
                normalized_optional(tag, ItemKey::TrackArtist).unwrap_or_default();
            snapshot.artists = normalized_values(tag, ItemKey::TrackArtists);
            if snapshot.artists.is_empty() && !snapshot.artist_credit.is_empty() {
                snapshot.artists.push(snapshot.artist_credit.clone());
            }
            snapshot.album_artists = normalized_values(tag, ItemKey::AlbumArtists);
            snapshot.composers = normalized_values(tag, ItemKey::Composer);
            snapshot.performers = normalized_values(tag, ItemKey::Performer);
            snapshot.compilation = normalized_optional(tag, ItemKey::FlagCompilation)
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
            snapshot.musicbrainz_release_id =
                normalized_optional(tag, ItemKey::MusicBrainzReleaseId);
            snapshot.musicbrainz_release_group_id =
                normalized_optional(tag, ItemKey::MusicBrainzReleaseGroupId);
            snapshot.musicbrainz_recording_id =
                normalized_optional(tag, ItemKey::MusicBrainzRecordingId);
            snapshot.musicbrainz_track_id = normalized_optional(tag, ItemKey::MusicBrainzTrackId);
            snapshot.musicbrainz_artist_ids = normalized_values(tag, ItemKey::MusicBrainzArtistId);
            snapshot.musicbrainz_album_artist_ids =
                normalized_values(tag, ItemKey::MusicBrainzReleaseArtistId);
            snapshot
        })
        .collect()
}

/// Should this file's per-track ARTIST be rewritten to the album artist?
///
/// ARTIST is TRACK scope — a release with guest performers legitimately holds a
/// different artist per track — but the editor offers exactly one artist field.
/// The DB index resolves that by renaming the track artist ONLY where it still
/// equals the value that was uniform across the album before the edit
/// (`Database::update_album_group_metadata`). The file writer has to agree with
/// it, or the two halves of one save disagree.
///
/// It used to call `set_artist(album_artist)` unconditionally on every file,
/// which meant a various-artists release — where the editor leaves the artist
/// field EMPTY precisely because the track artists disagree — had an empty
/// ARTIST written over every one of its files. Irreversibly.
fn should_rename_artist(
    current: Option<&str>,
    previous_uniform: Option<&str>,
    new_artist: &str,
) -> bool {
    if new_artist.trim().is_empty() {
        return false;
    }
    let Some(previous) = previous_uniform.map(str::trim).filter(|p| !p.is_empty()) else {
        // The album had no single prior artist, so there is no value this edit
        // can be said to be renaming. Leave every file's ARTIST alone.
        return false;
    };
    current.map(str::trim) == Some(previous)
}

/// The artist shared by every file, or `None` when they disagree or any is
/// blank. Read from the FILES rather than the library index, because the files
/// are what is about to be overwritten.
fn uniform_file_artist(paths: &[&str]) -> Option<String> {
    use lofty::prelude::*;

    let mut shared: Option<String> = None;
    for path in paths {
        let file = lofty::read_from_path(Path::new(path)).ok()?;
        let tag = file.primary_tag().or_else(|| file.first_tag())?;
        let artist = tag.artist()?.trim().to_string();
        if artist.is_empty() {
            return None;
        }
        match &shared {
            None => shared = Some(artist),
            Some(seen) if *seen == artist => {}
            Some(_) => return None,
        }
    }
    shared
}

fn apply_editor_fields(
    tag: &mut lofty::tag::Tag,
    album: &AlbumTagWrite,
    track: &TrackTagWrite,
    previous_artist: Option<&str>,
) {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;

    tag.set_title(track.title.trim().to_string());
    tag.set_album(album.album_title.trim().to_string());

    // ARTIST is track scope — see `should_rename_artist`.
    let current_artist = tag.artist().map(|artist| artist.into_owned());
    if should_rename_artist(
        current_artist.as_deref(),
        previous_artist,
        &album.album_artist,
    ) {
        tag.set_artist(album.album_artist.trim().to_string());
    }

    match track.track_number {
        Some(number) => tag.set_track(number),
        None => tag.remove_track(),
    }
    match track.disc_number {
        Some(number) => tag.set_disk(number),
        None => tag.remove_disk(),
    }

    if album.album_artist.trim().is_empty() {
        tag.remove_key(ItemKey::AlbumArtist);
    } else {
        tag.insert_text(ItemKey::AlbumArtist, album.album_artist.trim().to_string());
    }

    if let Some(year) = album.year {
        tag.set_date(lofty::tag::items::Timestamp {
            year: year as u16,
            ..Default::default()
        });
    } else {
        tag.remove_date();
    }

    if let Some(genre) = album
        .genre
        .as_deref()
        .map(str::trim)
        .filter(|genre| !genre.is_empty())
    {
        tag.set_genre(genre.to_string());
    } else {
        tag.remove_genre();
    }

    if let Some(catalog) = album
        .catalog_number
        .as_deref()
        .map(str::trim)
        .filter(|catalog| !catalog.is_empty())
    {
        tag.insert_text(ItemKey::CatalogNumber, catalog.to_string());
    } else {
        tag.remove_key(ItemKey::CatalogNumber);
    }
}

fn replace_text_values(tag: &mut lofty::tag::Tag, key: lofty::tag::ItemKey, values: &[String]) {
    use lofty::tag::{ItemValue, TagItem};

    tag.remove_key(key);
    for value in values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        tag.push(TagItem::new(key, ItemValue::Text(value.to_string())));
    }
}

fn replace_optional_text(tag: &mut lofty::tag::Tag, key: lofty::tag::ItemKey, value: Option<&str>) {
    let values = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default();
    replace_text_values(tag, key, &values);
}

fn apply_extended_editor_fields(
    tag: &mut lofty::tag::Tag,
    album: &ExtendedAlbumTagWrite,
    track: &ExtendedTrackTagWrite,
    artwork: Option<&lofty::picture::Picture>,
) {
    use lofty::picture::PictureType;
    use lofty::prelude::*;
    use lofty::tag::ItemKey;

    if !track.artist_credit.trim().is_empty() {
        tag.set_artist(track.artist_credit.trim().to_string());
    } else {
        tag.remove_key(ItemKey::TrackArtist);
    }
    replace_text_values(tag, ItemKey::TrackArtists, &track.artists);
    replace_text_values(tag, ItemKey::AlbumArtists, &album.album_artists);
    replace_text_values(tag, ItemKey::Composer, &track.composers);
    replace_text_values(tag, ItemKey::Performer, &track.performers);
    match album.compilation {
        Some(true) => replace_optional_text(tag, ItemKey::FlagCompilation, Some("1")),
        Some(false) => tag.remove_key(ItemKey::FlagCompilation),
        None => {}
    }
    replace_optional_text(
        tag,
        ItemKey::MusicBrainzReleaseId,
        album.musicbrainz_release_id.as_deref(),
    );
    replace_optional_text(
        tag,
        ItemKey::MusicBrainzReleaseGroupId,
        album.musicbrainz_release_group_id.as_deref(),
    );
    replace_text_values(
        tag,
        ItemKey::MusicBrainzReleaseArtistId,
        &album.musicbrainz_album_artist_ids,
    );
    replace_optional_text(
        tag,
        ItemKey::MusicBrainzRecordingId,
        track.musicbrainz_recording_id.as_deref(),
    );
    replace_optional_text(
        tag,
        ItemKey::MusicBrainzTrackId,
        track.musicbrainz_track_id.as_deref(),
    );
    replace_text_values(
        tag,
        ItemKey::MusicBrainzArtistId,
        &track.musicbrainz_artist_ids,
    );
    if let Some(picture) = artwork {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(picture.clone());
    }
}

fn normalized_tag_text(tag: &lofty::tag::Tag, key: lofty::tag::ItemKey) -> Option<String> {
    tag.get_string(key)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn verify_editor_write(
    path: &Path,
    album: &AlbumTagWrite,
    track: &TrackTagWrite,
    expected_artist: Option<&str>,
    extended_album: Option<&ExtendedAlbumTagWrite>,
    extended_track: Option<&ExtendedTrackTagWrite>,
    expected_artwork: Option<&[u8]>,
) -> Result<(), LibraryError> {
    use lofty::prelude::*;
    use lofty::tag::ItemKey;

    let tagged_file = lofty::read_from_path(path).map_err(|error| {
        LibraryError::Metadata(format!(
            "Tags were written but could not be verified in {}: {error}",
            path.display()
        ))
    })?;
    let tag = tagged_file.primary_tag().ok_or_else(|| {
        LibraryError::Metadata(format!(
            "The canonical tag was missing after writing {}.",
            path.display()
        ))
    })?;

    let actual_year = tag.date().map(|date| date.year as u32);
    let expected_genre = album
        .genre
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expected_catalog = album
        .catalog_number
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expected_album_artist =
        (!album.album_artist.trim().is_empty()).then(|| album.album_artist.trim().to_string());

    let mut matches = tag.title().as_deref().map(str::trim) == Some(track.title.trim())
        && tag.album().as_deref().map(str::trim) == Some(album.album_title.trim())
        && tag.track() == track.track_number
        && tag.disk() == track.disc_number
        && actual_year == album.year
        && tag.genre().as_deref().map(str::trim).map(str::to_string) == expected_genre
        && normalized_tag_text(tag, ItemKey::AlbumArtist) == expected_album_artist
        && normalized_tag_text(tag, ItemKey::CatalogNumber) == expected_catalog
        && expected_artist.map_or(true, |artist| {
            tag.artist().as_deref().map(str::trim) == Some(artist)
        });
    if let (Some(album), Some(track)) = (extended_album, extended_track) {
        let supports = |key: ItemKey| key.map_key(tag.tag_type()).is_some();
        let values_match = |key: ItemKey, expected: &[String]| {
            !supports(key) || normalized_values(tag, key) == expected
        };
        let option_match = |key: ItemKey, expected: Option<&str>| {
            !supports(key)
                || normalized_optional(tag, key).as_deref()
                    == expected.map(str::trim).filter(|value| !value.is_empty())
        };
        matches = matches
            && tag.artist().as_deref().map(str::trim) == Some(track.artist_credit.trim())
            && values_match(ItemKey::TrackArtists, &track.artists)
            && values_match(ItemKey::AlbumArtists, &album.album_artists)
            && values_match(ItemKey::Composer, &track.composers)
            && values_match(ItemKey::Performer, &track.performers)
            && option_match(
                ItemKey::MusicBrainzReleaseId,
                album.musicbrainz_release_id.as_deref(),
            )
            && option_match(
                ItemKey::MusicBrainzReleaseGroupId,
                album.musicbrainz_release_group_id.as_deref(),
            )
            && values_match(
                ItemKey::MusicBrainzReleaseArtistId,
                &album.musicbrainz_album_artist_ids,
            )
            && option_match(
                ItemKey::MusicBrainzRecordingId,
                track.musicbrainz_recording_id.as_deref(),
            )
            && option_match(
                ItemKey::MusicBrainzTrackId,
                track.musicbrainz_track_id.as_deref(),
            )
            && values_match(ItemKey::MusicBrainzArtistId, &track.musicbrainz_artist_ids);
        if let Some(compilation) = album.compilation {
            matches = matches
                && (!supports(ItemKey::FlagCompilation)
                    || normalized_optional(tag, ItemKey::FlagCompilation)
                        .map(|value| {
                            matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
                        })
                        .unwrap_or(false)
                        == compilation);
        }
    }
    if let Some(bytes) = expected_artwork {
        use lofty::picture::PictureType;
        matches = matches
            && tag
                .get_picture_type(PictureType::CoverFront)
                .map(|picture| picture.data())
                == Some(bytes);
    }
    if matches {
        Ok(())
    } else {
        Err(LibraryError::Metadata(format!(
            "Tag verification failed for {}. The library index was not updated.",
            path.display()
        )))
    }
}

/// Standards-aware direct embedded-tag writer.
///
/// The operation deduplicates by path, preflights **every** member for
/// existence, read/write access, parseability and a writable canonical tag
/// before touching the first file, then verifies the canonical values after
/// every save. An I/O failure can still leave earlier files changed — no audio
/// format offers a cross-file transaction — but predictable permission,
/// format and missing-mount failures are caught before that point.
///
/// Existing secondary layers are preserved. With
/// `synchronize_secondary_tags`, existing writable modern layers are updated
/// too; ID3v1 stays untouched because its 30-byte/Latin-1 limits would silently
/// truncate or corrupt values that are valid in the canonical tag.
fn write_album_tags_impl(
    album: &AlbumTagWrite,
    extended_album: &ExtendedAlbumTagWrite,
    tracks: &[TrackTagWrite],
    extended_tracks: &[ExtendedTrackTagWrite],
    front_cover: Option<&FrontCoverWrite>,
    apply_extended: bool,
    options: DirectTagWriteOptions,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<(), LibraryError> {
    use lofty::config::WriteOptions;
    use lofty::prelude::*;
    use lofty::tag::{Tag, TagType};

    let extended_by_path = extended_tracks
        .iter()
        .map(|track| (track.file_path.as_str(), track))
        .collect::<HashMap<_, _>>();
    if apply_extended
        && tracks
            .iter()
            .any(|track| !extended_by_path.contains_key(track.file_path.as_str()))
    {
        return Err(LibraryError::Metadata(
            "The extended tag rows do not match the audio rows.".to_string(),
        ));
    }
    let artwork = front_cover
        .map(|cover| {
            let mut picture = lofty::picture::Picture::from_reader(&mut cover.bytes.as_slice())
                .map_err(|error| {
                    LibraryError::Metadata(format!(
                        "The selected cover is not a supported image: {error}"
                    ))
                })?;
            picture.set_pic_type(lofty::picture::PictureType::CoverFront);
            Ok::<_, LibraryError>(picture)
        })
        .transpose()?;

    let mut seen = HashSet::new();
    let unique: Vec<&TrackTagWrite> = tracks
        .iter()
        .filter(|track| seen.insert(track.file_path.clone()))
        .collect();
    if unique.is_empty() {
        return Err(LibraryError::Metadata(
            "No audio files were provided for direct tag writing.".to_string(),
        ));
    }

    // Read the prior per-track artist BEFORE any edit, so the rename rule sees
    // one coherent album rather than the progressively modified loop state.
    let previous_artist = if album.album_artist.trim().is_empty() {
        None
    } else {
        let paths: Vec<&str> = unique
            .iter()
            .map(|track| track.file_path.as_str())
            .collect();
        uniform_file_artist(&paths)
    };

    // Full preflight + in-memory mutation. The write loop below therefore has
    // no expected missing-file, permission, parse or unsupported-tag failure.
    let mut prepared = Vec::with_capacity(unique.len());
    for track in unique {
        let path = Path::new(&track.file_path);
        if !path.is_file() {
            return Err(LibraryError::Metadata(format!(
                "Audio file not found: {}",
                path.display()
            )));
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| {
                LibraryError::Metadata(format!(
                    "Audio file is not writable ({}): {error}",
                    path.display()
                ))
            })?;

        let mut tagged_file = lofty::read_from_path(path).map_err(|error| {
            LibraryError::Metadata(format!(
                "Failed to read audio tags in {}: {error}",
                path.display()
            ))
        })?;
        let primary_type = tagged_file.primary_tag_type();
        if !tagged_file.tag_support(primary_type).is_writable() {
            return Err(LibraryError::Metadata(format!(
                "{} does not have a writable canonical tag for this format.",
                path.display()
            )));
        }

        let expected_artist = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
            .and_then(|tag| tag.artist())
            .map(|artist| artist.into_owned())
            .filter(|artist| {
                should_rename_artist(
                    Some(artist.as_str()),
                    previous_artist.as_deref(),
                    &album.album_artist,
                )
            })
            .map(|_| album.album_artist.trim().to_string());

        if tagged_file.primary_tag().is_none() {
            tagged_file.insert_tag(Tag::new(primary_type));
        }

        let mut targets = vec![primary_type];
        if options.synchronize_secondary_tags {
            for tag in tagged_file.tags() {
                let tag_type = tag.tag_type();
                if tag_type != primary_type
                    && tag_type != TagType::Id3v1
                    && tagged_file.tag_support(tag_type).is_writable()
                    && !targets.contains(&tag_type)
                {
                    targets.push(tag_type);
                }
            }
        }
        for tag_type in targets {
            let tag = tagged_file.tag_mut(tag_type).ok_or_else(|| {
                LibraryError::Metadata(format!(
                    "Failed to access {} in {}.",
                    tag_type_name(tag_type),
                    path.display()
                ))
            })?;
            apply_editor_fields(tag, album, track, previous_artist.as_deref());
            if apply_extended {
                let extended_track = extended_by_path
                    .get(track.file_path.as_str())
                    .expect("validated extended editor row");
                apply_extended_editor_fields(tag, extended_album, extended_track, artwork.as_ref());
            }
        }

        prepared.push((track, tagged_file, expected_artist));
    }

    let total = prepared.len();
    let mut write_options = WriteOptions::new();
    write_options.use_id3v23(options.id3v2_version == Id3v2WriteVersion::V23);
    // Never turn a valid Unicode tag into '?' merely because a secondary
    // layer cannot represent it. The canonical formats all support Unicode.
    write_options.lossy_text_encoding(false);

    for (index, (track, tagged_file, expected_artist)) in prepared.into_iter().enumerate() {
        let path = Path::new(&track.file_path);
        tagged_file
            .save_to_path(path, write_options)
            .map_err(|error| {
                LibraryError::Metadata(format!(
                    "Failed to write tags to {}: {error}",
                    path.display()
                ))
            })?;
        let extended_track = apply_extended.then(|| {
            extended_by_path
                .get(track.file_path.as_str())
                .copied()
                .expect("validated extended editor row")
        });
        verify_editor_write(
            path,
            album,
            track,
            expected_artist.as_deref(),
            apply_extended.then_some(extended_album),
            extended_track,
            apply_extended
                .then(|| front_cover.map(|cover| cover.bytes.as_slice()))
                .flatten(),
        )?;
        on_progress(index + 1, total);
    }

    Ok(())
}

/// Write the editor's canonical display fields, ordered artist components,
/// credits, standard MusicBrainz identifiers and an optional front cover.
pub fn write_album_tags_to_files_extended(
    album: &AlbumTagWrite,
    extended_album: &ExtendedAlbumTagWrite,
    tracks: &[TrackTagWrite],
    extended_tracks: &[ExtendedTrackTagWrite],
    front_cover: Option<&FrontCoverWrite>,
    options: DirectTagWriteOptions,
    on_progress: impl FnMut(usize, usize),
) -> Result<(), LibraryError> {
    write_album_tags_impl(
        album,
        extended_album,
        tracks,
        extended_tracks,
        front_cover,
        true,
        options,
        on_progress,
    )
}

pub fn write_album_tags_to_files_with_options(
    album: &AlbumTagWrite,
    tracks: &[TrackTagWrite],
    options: DirectTagWriteOptions,
    on_progress: impl FnMut(usize, usize),
) -> Result<(), LibraryError> {
    let extended_tracks = tracks
        .iter()
        .map(|track| ExtendedTrackTagWrite {
            file_path: track.file_path.clone(),
            ..ExtendedTrackTagWrite::default()
        })
        .collect::<Vec<_>>();
    write_album_tags_impl(
        album,
        &ExtendedAlbumTagWrite::default(),
        tracks,
        &extended_tracks,
        None,
        false,
        options,
        on_progress,
    )
}

/// Backwards-compatible default used by the frozen Slint frontend: canonical
/// tag, ID3v2.4, secondary layers preserved.
pub fn write_album_tags_to_files(
    album: &AlbumTagWrite,
    tracks: &[TrackTagWrite],
    on_progress: impl FnMut(usize, usize),
) -> Result<(), LibraryError> {
    write_album_tags_to_files_with_options(
        album,
        tracks,
        DirectTagWriteOptions::default(),
        on_progress,
    )
}

/// Returns `Some(v)` iff every non-blank track shares one
/// `album_artist ?? artist`, else `None`. Empty / all-blank => `None`.
/// Port of the Tauri `library_compute_track_artist_match`.
pub fn compute_track_artist_match(tracks: &[LocalTrack]) -> Option<String> {
    let mut artists: std::collections::HashSet<String> = std::collections::HashSet::new();
    for track in tracks {
        let value = track
            .album_artist
            .as_deref()
            .unwrap_or(track.artist.as_str())
            .trim();
        if value.is_empty() {
            continue;
        }
        artists.insert(value.to_string());
        if artists.len() > 1 {
            return None;
        }
    }
    artists.into_iter().next()
}

// ─── Purchase downloads ──────────────────────────────────────────────────────

/// Everything a freshly downloaded purchased track should carry in its embedded
/// tags. Every field comes from the API payload the download already fetched
/// (`/track/get` runs before every purchase download), so nothing here is
/// guessed from the file.
#[derive(Debug, Clone, Default)]
pub struct PurchaseTagWrite {
    pub title: String,
    /// Track subtitle ("Live", "Remastered"). Written to its own tag rather than
    /// glued onto TITLE, so the value survives losslessly and TITLE keeps
    /// matching the on-disk filename.
    pub version: Option<String>,
    /// The track's OWN performer. This is the field that makes a purpose-built
    /// writer necessary — see [`write_purchase_tags`].
    pub artist: String,
    pub album_title: String,
    pub album_artist: String,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub label: Option<String>,
    pub isrc: Option<String>,
    pub composer: Option<String>,
    pub copyright: Option<String>,
}

/// Tag a freshly downloaded purchased file, optionally embedding cover art.
///
/// **Why this is not `write_album_tags_to_files`.** That writer serves the
/// metadata EDITOR, and two of its defining behaviours are wrong here:
///
///  1. It never writes ARTIST outright. It renames the track artist only where
///     the file's existing value still matches the album's previously-uniform
///     one (`should_rename_artist`), and it derives that prior value by READING
///     the files. A just-downloaded file has no tags at all, so the uniform
///     lookup yields `None`, the rename is declined, and every purchased track
///     would land with NO artist. A purchase knows the real per-track performer
///     from the API, so it writes it directly and skips the rename machinery.
///  2. It is destructive by design — a `None` field REMOVES that tag, which is
///     what an editor means by clearing a field. A download has nothing to
///     clear, so absent fields are skipped instead.
///
/// It is also single-file and independent: the album loop tags each track as it
/// lands, so a cancelled or partly-failed album still leaves correct tags on
/// whatever did download.
///
/// **Failure is never fatal to the download.** The file is the deliverable; tags
/// are an enhancement over the reference, which wrote none at all. Callers log
/// the error and move on.
pub fn write_purchase_tags(
    file_path: &str,
    meta: &PurchaseTagWrite,
    cover_jpeg: Option<&[u8]>,
) -> Result<(), LibraryError> {
    use lofty::config::WriteOptions;
    use lofty::picture::{MimeType, Picture, PictureType};
    use lofty::prelude::*;
    use lofty::tag::{ItemKey, Tag};

    let path = Path::new(file_path);
    let mut tagged_file = lofty::read_from_path(path)
        .map_err(|e| LibraryError::Metadata(format!("Failed to read downloaded file tags: {e}")))?;

    // ALWAYS the format's PRIMARY tag type — never a "first tag we can find"
    // fallback. This is the difference between tagging the file and silently
    // tagging nothing:
    //
    // lofty will not WRITE a tag type a container only supports READING. A FLAC
    // may legitimately arrive carrying an ID3v2 tag; for such a file
    // `primary_tag_mut()` (VorbisComments) is `None` while `first_tag_mut()` is
    // `Some(id3v2)`. A fallback would happily fill that ID3v2 tag, and then
    // `save_to_path` would SKIP it — `tagged_file.rs` does `continue` on any
    // non-writable tag rather than erroring, precisely because callers are not
    // expected to know which ones are read-only. The result is a file with none
    // of its tags and a `Ok(())` return: no log line, no failure, nothing to
    // notice. The same trap costs six fields plus all artwork on an MP3 that
    // carries only ID3v1, whose key set is eight entries wide.
    //
    // Inserting the primary type is safe unconditionally — `insert_tag` replaces
    // any same-type tag — but it is done only when absent so an existing tag's
    // unrelated fields survive.
    let primary_type = tagged_file.primary_tag_type();
    if tagged_file.primary_tag_mut().is_none() {
        tagged_file.insert_tag(Tag::new(primary_type));
    }

    {
        let tag = tagged_file.primary_tag_mut().ok_or_else(|| {
            LibraryError::Metadata("Failed to access downloaded file tags.".to_string())
        })?;

        let set_text = |tag: &mut Tag, key: ItemKey, value: Option<&str>| {
            if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
                tag.insert_text(key, value.to_string());
            }
        };

        // Empties are SKIPPED, not written. A download has nothing to clear, and
        // an explicit empty TITLE/ALBUM is worse than an absent one — it defeats
        // any later repair pass that looks for missing tags.
        if !meta.title.trim().is_empty() {
            tag.set_title(meta.title.trim().to_string());
        }
        if !meta.album_title.trim().is_empty() {
            tag.set_album(meta.album_title.trim().to_string());
        }

        // Written unconditionally, from the API — never inferred from the file.
        if !meta.artist.trim().is_empty() {
            tag.set_artist(meta.artist.trim().to_string());
        }

        // `Track.track_number` is a non-optional `u32` that DEFAULTS to 0 when
        // the API omits it, so an unguarded write stamps `TRACKNUMBER=0` on
        // singles and unnumbered releases. Zero is not a track number.
        if let Some(no) = meta.track_number.filter(|n| *n > 0) {
            tag.set_track(no);
        }
        if let Some(disc) = meta.disc_number.filter(|d| *d > 0) {
            tag.set_disk(disc);
        }
        if let Some(year) = meta.year {
            tag.set_date(lofty::tag::items::Timestamp {
                year: year as u16,
                ..Default::default()
            });
        }
        if let Some(genre) = meta
            .genre
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
        {
            tag.set_genre(genre.to_string());
        }

        set_text(tag, ItemKey::AlbumArtist, Some(meta.album_artist.as_str()));
        set_text(tag, ItemKey::TrackSubtitle, meta.version.as_deref());
        set_text(tag, ItemKey::Label, meta.label.as_deref());
        set_text(tag, ItemKey::Isrc, meta.isrc.as_deref());
        set_text(tag, ItemKey::Composer, meta.composer.as_deref());
        set_text(tag, ItemKey::CopyrightMessage, meta.copyright.as_deref());

        if let Some(bytes) = cover_jpeg.filter(|b| !b.is_empty()) {
            // Drop any existing embedded art before pushing. On the download path
            // the file is always brand new, so today this is a no-op — but
            // `push_picture` APPENDS, so the moment anything re-tags an existing
            // file (a retry against a file the rename did not replace, or a
            // future re-tag action) the art would accumulate a second copy and
            // bloat every track by the size of the cover.
            //
            // BOTH types are removed, not just `CoverFront`: vendor-tagged files
            // commonly store the front cover as `Other`, and clearing only
            // `CoverFront` would leave that one in place and produce exactly the
            // duplicate this guard exists to prevent.
            tag.remove_picture_type(PictureType::CoverFront);
            tag.remove_picture_type(PictureType::Other);
            // `unchecked` skips lofty's format sniffing: the bytes come from the
            // album's `image.large`, which Qobuz serves as JPEG, and the mime is
            // declared rather than inferred.
            tag.push_picture(
                Picture::unchecked(bytes.to_vec())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Jpeg)
                    .build(),
            );
        }
    }

    tagged_file
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| LibraryError::Metadata(format!("Failed to write downloaded file tags: {e}")))
}

#[cfg(test)]
mod editor_write_target_tests {
    use super::*;

    /// A FLAC that arrives carrying an ID3v2 tag — an ordinary shape, and the
    /// one that exposed a silent data-loss bug in the metadata editor.
    fn flac_with_leading_id3v2() -> Vec<u8> {
        let mut frame = b"TIT2".to_vec();
        let text = b"\x03OldTitle"; // 0x03 = UTF-8
        frame.extend_from_slice(&(text.len() as u32).to_be_bytes());
        frame.extend_from_slice(&[0x00, 0x00]);
        frame.extend_from_slice(text);

        let size = frame.len() as u32;
        let syncsafe = [
            ((size >> 21) & 0x7F) as u8,
            ((size >> 14) & 0x7F) as u8,
            ((size >> 7) & 0x7F) as u8,
            (size & 0x7F) as u8,
        ];

        let mut out = b"ID3".to_vec();
        out.extend_from_slice(&[0x04, 0x00, 0x00]);
        out.extend_from_slice(&syncsafe);
        out.extend_from_slice(&frame);

        // A minimal but valid FLAC: the marker plus one final STREAMINFO block.
        out.extend_from_slice(b"fLaC");
        out.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        let mut info = vec![0u8; 34];
        info[0..2].copy_from_slice(&4096u16.to_be_bytes());
        info[2..4].copy_from_slice(&4096u16.to_be_bytes());
        let sample_rate: u32 = 44_100;
        info[10] = (sample_rate >> 12) as u8;
        info[11] = ((sample_rate >> 4) & 0xFF) as u8;
        info[12] = (((sample_rate & 0x0F) as u8) << 4) | (1 << 1);
        info[13] = 0xF0;
        out.extend_from_slice(&info);
        out
    }

    /// The metadata editor must write into the tag type the container can
    /// actually SAVE, not merely into the first tag the file happens to carry.
    ///
    /// This is the regression for a silent data-loss bug: the write target used
    /// to fall back to `first_tag_mut()`, which for this file is the ID3v2 tag.
    /// lofty refuses to write ID3v2 to FLAC and skips it WITHOUT erroring, so
    /// every edited field was discarded while the call returned `Ok(())` — the
    /// user pressed Save, got a success toast, and the file never changed.
    ///
    /// Verified to fail against the old target selection before being accepted.
    #[test]
    fn editing_a_flac_that_carries_id3v2_actually_writes() {
        use lofty::prelude::*;

        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("legacy.flac");
        std::fs::write(&file, flac_with_leading_id3v2()).unwrap();

        // The fixture must genuinely present the trap, or the test proves nothing.
        let before = lofty::read_from_path(&file).expect("readable FLAC fixture");
        assert!(
            before.tag(lofty::tag::TagType::Id3v2).is_some(),
            "fixture must carry ID3v2"
        );
        assert!(
            before.tag(lofty::tag::TagType::VorbisComments).is_none(),
            "fixture must not already have Vorbis comments"
        );

        let album = AlbumTagWrite {
            album_title: "Edited Album".to_string(),
            album_artist: "Edited Artist".to_string(),
            year: Some(1999),
            genre: Some("Jazz".to_string()),
            catalog_number: None,
        };
        let tracks = vec![TrackTagWrite {
            file_path: file.to_string_lossy().to_string(),
            title: "Edited Title".to_string(),
            track_number: Some(4),
            disc_number: Some(2),
        }];

        write_album_tags_to_files(&album, &tracks, |_, _| {}).expect("the edit must succeed");

        let after = lofty::read_from_path(&file).expect("re-read");
        let vorbis = after
            .tag(lofty::tag::TagType::VorbisComments)
            .expect("the edit must land in VORBIS COMMENTS — the type FLAC can write");

        assert_eq!(vorbis.title().as_deref(), Some("Edited Title"));
        assert_eq!(vorbis.album().as_deref(), Some("Edited Album"));
        assert_eq!(vorbis.track(), Some(4));
        assert_eq!(vorbis.disk(), Some(2));
        assert_eq!(vorbis.genre().as_deref(), Some("Jazz"));
    }

    #[test]
    fn inspection_exposes_canonical_and_secondary_layers_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("legacy.flac");
        std::fs::write(&file, flac_with_leading_id3v2()).unwrap();

        let inspection = inspect_album_tag_layers(&[file.to_string_lossy().to_string()]).unwrap();
        assert_eq!(inspection.file_count, 1);
        assert_eq!(inspection.writable_files, 1);
        assert!(inspection.direct_write_supported());
        assert_eq!(inspection.canonical_layers.len(), 1);
        assert_eq!(inspection.canonical_layers[0].name, "Vorbis comments");
        assert_eq!(inspection.canonical_layers[0].file_count, 0);
        assert_eq!(inspection.canonical_layers[0].writable_file_count, 1);
        assert_eq!(inspection.present_layers.len(), 1);
        assert_eq!(inspection.present_layers[0].name, "ID3v2");
        assert_eq!(inspection.present_layers[0].writable_file_count, 0);
    }

    #[test]
    fn canonical_write_preserves_secondary_layer_and_reports_its_conflict() {
        use lofty::prelude::*;

        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("legacy.flac");
        std::fs::write(&file, flac_with_leading_id3v2()).unwrap();
        let album = AlbumTagWrite {
            album_title: "Edited Album".to_string(),
            album_artist: "Edited Artist".to_string(),
            year: Some(1999),
            genre: None,
            catalog_number: None,
        };
        let tracks = vec![TrackTagWrite {
            file_path: file.to_string_lossy().to_string(),
            title: "Edited Title".to_string(),
            track_number: Some(1),
            disc_number: Some(1),
        }];

        write_album_tags_to_files(&album, &tracks, |_, _| {}).unwrap();
        let after = lofty::read_from_path(&file).unwrap();
        assert_eq!(
            after
                .tag(lofty::tag::TagType::Id3v2)
                .and_then(|tag| tag.title())
                .as_deref(),
            Some("OldTitle")
        );
        assert_eq!(
            after
                .tag(lofty::tag::TagType::VorbisComments)
                .and_then(|tag| tag.title())
                .as_deref(),
            Some("Edited Title")
        );

        let inspection = inspect_album_tag_layers(&[file.to_string_lossy().to_string()]).unwrap();
        assert_eq!(inspection.conflicting_files, 1);
        assert_eq!(inspection.present_layers.len(), 2);
    }

    #[test]
    fn clearing_track_and_disc_numbers_is_persisted_and_verified() {
        use lofty::prelude::*;

        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("legacy.flac");
        std::fs::write(&file, flac_with_leading_id3v2()).unwrap();
        let album = AlbumTagWrite {
            album_title: "Edited Album".to_string(),
            album_artist: String::new(),
            year: None,
            genre: None,
            catalog_number: None,
        };
        let mut tracks = vec![TrackTagWrite {
            file_path: file.to_string_lossy().to_string(),
            title: "Edited Title".to_string(),
            track_number: Some(9),
            disc_number: Some(3),
        }];
        write_album_tags_to_files(&album, &tracks, |_, _| {}).unwrap();

        tracks[0].track_number = None;
        tracks[0].disc_number = None;
        let mut progress = Vec::new();
        write_album_tags_to_files(&album, &tracks, |current, total| {
            progress.push((current, total));
        })
        .unwrap();

        let after = lofty::read_from_path(&file).unwrap();
        let tag = after.primary_tag().unwrap();
        assert_eq!(tag.track(), None);
        assert_eq!(tag.disk(), None);
        assert_eq!(progress, vec![(1, 1)]);
    }

    #[test]
    fn rich_writer_preserves_display_credit_components_ids_and_front_cover() {
        use lofty::picture::PictureType;
        use lofty::prelude::*;
        use std::io::Cursor;

        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("credits.flac");
        std::fs::write(&file, flac_with_leading_id3v2()).unwrap();
        let mut cover = Vec::new();
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut Cursor::new(&mut cover), image::ImageFormat::Png)
            .unwrap();
        let album = AlbumTagWrite {
            album_title: "Compilation".to_string(),
            album_artist: "Various Artists".to_string(),
            year: Some(2026),
            genre: Some("Soundtrack".to_string()),
            catalog_number: Some("CAT-42".to_string()),
        };
        let extended_album = ExtendedAlbumTagWrite {
            album_artists: vec!["Various Artists".to_string()],
            compilation: Some(true),
            musicbrainz_release_id: Some("release-id".to_string()),
            musicbrainz_release_group_id: Some("group-id".to_string()),
            musicbrainz_album_artist_ids: vec!["va-id".to_string()],
            discogs_release_id: Some("123".to_string()),
        };
        let tracks = vec![TrackTagWrite {
            file_path: file.to_string_lossy().to_string(),
            title: "Collaboration".to_string(),
            track_number: Some(1),
            disc_number: Some(1),
        }];
        let rich_tracks = vec![ExtendedTrackTagWrite {
            file_path: tracks[0].file_path.clone(),
            artist_credit: "Alpha feat. Beta".to_string(),
            artists: vec!["Alpha".to_string(), "Beta".to_string()],
            composers: vec!["Composer One".to_string()],
            performers: vec!["Player One (guitar)".to_string()],
            musicbrainz_recording_id: Some("recording-id".to_string()),
            musicbrainz_track_id: Some("track-id".to_string()),
            musicbrainz_artist_ids: vec!["alpha-id".to_string(), "beta-id".to_string()],
        }];

        write_album_tags_to_files_extended(
            &album,
            &extended_album,
            &tracks,
            &rich_tracks,
            Some(&FrontCoverWrite {
                bytes: cover.clone(),
            }),
            DirectTagWriteOptions::default(),
            |_, _| {},
        )
        .unwrap();

        let snapshot = read_editor_tag_snapshots(
            &tracks
                .iter()
                .map(|t| t.file_path.clone())
                .collect::<Vec<_>>(),
        );
        assert_eq!(snapshot[0].artist_credit, "Alpha feat. Beta");
        assert_eq!(snapshot[0].artists, ["Alpha", "Beta"]);
        assert_eq!(snapshot[0].album_artists, ["Various Artists"]);
        assert_eq!(snapshot[0].composers, ["Composer One"]);
        assert_eq!(
            snapshot[0].musicbrainz_release_id.as_deref(),
            Some("release-id")
        );
        assert_eq!(
            snapshot[0].musicbrainz_recording_id.as_deref(),
            Some("recording-id")
        );
        assert_eq!(snapshot[0].musicbrainz_artist_ids, ["alpha-id", "beta-id"]);
        let after = lofty::read_from_path(&file).unwrap();
        assert_eq!(
            after
                .primary_tag()
                .and_then(|tag| tag.get_picture_type(PictureType::CoverFront))
                .map(|picture| picture.data()),
            Some(cover.as_slice())
        );
    }

    #[test]
    fn folder_cover_rejects_invalid_bytes_without_replacing_existing_art() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("cover.jpg");
        std::fs::write(&existing, b"existing cover").unwrap();

        assert!(write_folder_front_cover(tmp.path(), b"not an image").is_err());
        assert_eq!(std::fs::read(existing).unwrap(), b"existing cover");
    }
}

#[cfg(test)]
mod tests {
    use super::should_rename_artist;

    #[test]
    fn renames_only_the_artist_the_album_actually_had() {
        // The ordinary case: one artist across the album, user renames it.
        assert!(should_rename_artist(
            Some("Seiji Yokoyama"),
            Some("Seiji Yokoyama"),
            "Yokoyama Seiji"
        ));
        // A guest track inside an otherwise uniform album keeps its performer.
        assert!(!should_rename_artist(
            Some("MAKE-UP"),
            Some("Seiji Yokoyama"),
            "Yokoyama Seiji"
        ));
    }

    #[test]
    fn never_blanks_the_track_artist() {
        // The various-artists shape: the editor leaves the field empty because
        // the track artists disagree. Writing that empty value over 247 files
        // is the data loss this guard exists to stop.
        assert!(!should_rename_artist(Some("MAKE-UP"), None, ""));
        assert!(!should_rename_artist(Some("MAKE-UP"), Some("MAKE-UP"), ""));
        assert!(!should_rename_artist(
            Some("MAKE-UP"),
            Some("MAKE-UP"),
            "   "
        ));
    }

    #[test]
    fn no_uniform_prior_artist_means_nothing_is_renamed() {
        assert!(!should_rename_artist(
            Some("MAKE-UP"),
            None,
            "Various Artists"
        ));
        assert!(!should_rename_artist(
            Some("MAKE-UP"),
            Some(""),
            "Various Artists"
        ));
        assert!(!should_rename_artist(
            None,
            Some("MAKE-UP"),
            "Yokoyama Seiji"
        ));
    }
}
