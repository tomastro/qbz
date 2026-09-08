//! Local Library transport rows + the `qbz_library` -> row mappers.
//!
//! Split out of `local_library_qt.rs` (phase-24 modularization). This file
//! owns ONLY the shape the QML view parses (one flat serde document per
//! surface) and the pure mapping helpers that fill it. No DB access, no
//! state, no Qt.
//!
//! The `source` field is the row's SOURCE BADGE value and has exactly three
//! values app-wide: `"local"` (a user file), `"offline"` (a Qobuz download —
//! the Slint's raw `qobuz_download`), `"plex"` (a Plex-cache row). QML keys
//! its badge glyph/tint off this string.
//!
//! `sourceRaw` rides ALONGSIDE it and is the exception that proves the fold is
//! lossy: a scanned file that matches a `downloaded_purchases` row is stamped
//! `qobuz_purchase` in the DB (`database.rs:1105-1120`), `source` folds that
//! into `"offline"` so it keeps filtering under the Offline chip, and
//! `sourceRaw` carries the word itself so the badge can draw the GOLD Qobuz
//! mark. Two fields because the filter and the badge want different answers
//! from the same column. It is emitted ONLY when it says something new, so on
//! a library with no purchases every published row is byte-identical to what
//! it was before.

use std::collections::HashMap;

use qbz_library::{AudioFormat, LocalAlbum, LocalTrack};
use qbz_local_catalog::AlbumRecord;
use qbz_source::SourceId;
use serde::Serialize;

// ---------------------------------------------------------------------------
// Rows (the QML contract)
// ---------------------------------------------------------------------------

/// One physical media variant represented by a logical album row. Keeping
/// source, format and quality together is load-bearing: the filter sections
/// are AND-ed, so three independent aggregate arrays could claim a match by
/// taking DSD from one copy and FLAC/local from another.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AlbumMediaVariant {
    #[serde(rename = "qualityTier")]
    pub quality_tier: String,
    pub format: String,
    pub source: String,
}

/// One album card (Albums tab + Folders tab flat mode). `id` is the group key
/// of the ACTIVE identity mode — folder or metadata — so the detail query can
/// round-trip it. Plex albums carry the content-hash key `plex:<hash>`.
#[derive(Clone, Default, Serialize)]
pub struct AlbumRow {
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(rename = "allArtists")]
    pub all_artists: String,
    /// Canonical individual contributors used by chained artist facets.
    pub artists: Vec<String>,
    pub year: String,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    pub duration: String,
    #[serde(rename = "qualityTier")]
    pub quality_tier: String,
    #[serde(rename = "qualityDetail")]
    pub quality_detail: String,
    pub format: String,
    /// Every physical quality/format/source combination represented by this
    /// logical card. The scalar fields above remain the default/highest
    /// quality version used by the visible badge.
    #[serde(rename = "mediaVariants")]
    pub media_variants: Vec<AlbumMediaVariant>,
    /// Every genre represented by this logical album, across all versions.
    pub genres: Vec<String>,
    #[serde(rename = "artKey")]
    pub art_key: String,
    /// "local" | "offline" | "plex" (the source badge).
    pub source: String,
    /// Every physical source represented by this logical album. Values stay
    /// in the raw badge vocabulary so a Qobuz purchase keeps its gold mark.
    #[serde(default)]
    pub sources: Vec<String>,
    /// `"qobuz_purchase"`, or empty (omitted from the JSON). See the module
    /// header: the badge prefers this, every filter keeps reading `source`.
    #[serde(rename = "sourceRaw", skip_serializing_if = "String::is_empty")]
    pub source_raw: String,
    #[serde(rename = "directoryPath")]
    pub directory_path: String,
    /// Number of distinct contributing folders (metadata mode only; > 1 is
    /// the "album spans N folders" tooltip case).
    #[serde(rename = "folderCount")]
    pub folder_count: u32,
    /// Heart state from the shared LocalFavoritesService.
    #[serde(rename = "isFavorite")]
    pub is_favorite: bool,
    /// True when the logical album belongs to the Local Library favorite
    /// domain: a genuine local copy or a configured media-server copy.
    /// Qobuz-offline-only rows stay in the catalog-favorite domain.
    pub favoriteable: bool,
}

/// One track row (Tracks tab, album detail, folder detail).
#[derive(Clone, Default, Serialize)]
pub struct TrackRow {
    /// The local-library row id as a string. Plex rows carry the namespaced
    /// id from `local_plex::PLEX_TRACK_ID_FLOOR`, so it can never collide
    /// with a real `local_tracks.id`.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    #[serde(rename = "albumId")]
    pub album_id: String,
    /// Never a Qobuz artist id — local/Plex rows have none; kept so the
    /// shared TrackRow.qml "go to artist" arm stays hidden.
    #[serde(rename = "artistId")]
    pub artist_id: String,
    pub number: u32,
    pub disc: u32,
    pub duration: String,
    #[serde(rename = "qualityTier")]
    pub quality_tier: String,
    #[serde(rename = "qualityDetail")]
    pub quality_detail: String,
    pub format: String,
    pub genres: Vec<String>,
    pub year: String,
    #[serde(rename = "artKey")]
    pub art_key: String,
    /// Filled by the artwork window (never on the bulk publish).
    #[serde(rename = "artPath")]
    pub art_path: String,
    /// "local" | "offline" | "plex" (the source badge).
    pub source: String,
    /// `"qobuz_purchase"`, or empty (omitted from the JSON). See the module
    /// header: the badge prefers this, every filter keeps reading `source`.
    #[serde(rename = "sourceRaw", skip_serializing_if = "String::is_empty")]
    pub source_raw: String,
    pub explicit: bool,
    #[serde(rename = "isFavorite")]
    pub is_favorite: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct ArtistRow {
    pub name: String,
    #[serde(rename = "albumCount")]
    pub album_count: u32,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    #[serde(rename = "artKey")]
    pub art_key: String,
    /// "local" | "plex" | "mixed" — the rail's provenance hint.
    pub source: String,
    /// Facets aggregated from every album credited to this artist. Keeping
    /// these on the bounded artist document lets the Artists toolbar reuse
    /// the Albums funnel without materialising tracks or guessing from the
    /// representative portrait source.
    pub sources: Vec<String>,
    pub formats: Vec<String>,
    #[serde(rename = "qualityTiers")]
    pub quality_tiers: Vec<String>,
    pub years: Vec<u32>,
    /// Most recent known release year; empty when the source supplied none.
    /// The reversible year sort is deliberately defined over one stable key.
    pub year: String,
}

/// One row of the FLATTENED folder tree (the rail renders a windowed list
/// over this array — never a recursive component).
#[derive(Clone, Default, Serialize)]
pub struct TreeNode {
    pub path: String,
    pub segment: String,
    pub depth: i32,
    #[serde(rename = "isFolder")]
    pub is_folder: bool,
    #[serde(rename = "canExpand")]
    pub can_expand: bool,
    pub expanded: bool,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    #[serde(rename = "artKey")]
    pub art_key: String,
}

#[derive(Clone, Default, Serialize)]
pub struct SubfolderRow {
    pub path: String,
    pub name: String,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    #[serde(rename = "artKey")]
    pub art_key: String,
}

#[derive(Clone, Default, Serialize)]
pub struct FolderDetail {
    pub path: String,
    pub name: String,
    #[serde(rename = "trackCount")]
    pub track_count: u32,
    pub subfolders: Vec<SubfolderRow>,
    pub tracks: Vec<TrackRow>,
}

#[derive(Clone, Default, Serialize)]
pub struct LocalCounts {
    pub albums: i64,
    pub artists: i64,
    pub folders: i64,
    pub tracks: i64,
    /// Cached Plex tracks folded into `tracks` (0 when Plex is off) — the
    /// header can show "N of them from Plex" without a second query.
    #[serde(rename = "plexTracks")]
    pub plex_tracks: i64,
    #[serde(rename = "jellyfinTracks")]
    pub jellyfin_tracks: i64,
    #[serde(rename = "subsonicTracks")]
    pub subsonic_tracks: i64,
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

pub fn mmss(secs: u64) -> String {
    let m = secs / 60;
    let s = secs % 60;
    if m >= 60 {
        format!("{}:{:02}:{:02}", m / 60, m % 60, s)
    } else {
        format!("{m}:{s:02}")
    }
}

/// Human total ("1 h 12 min" / "42 min").
pub fn total_duration(secs: u64) -> String {
    let mins = secs / 60;
    if mins >= 60 {
        format!("{} h {} min", mins / 60, mins % 60)
    } else {
        format!("{mins} min")
    }
}

/// Stable ordering for physical versions. DSD is its own top-level tier:
/// treating its nominal one-bit depth as ordinary PCM made DSD64 lose to
/// 24/96 even though the UI and version picker expose DSD as the premium
/// representation. Within a tier, depth then rate preserve the existing PCM
/// ordering; within DSD, the common depth is equal so the DSD rate decides.
pub fn media_quality_rank(
    format: &AudioFormat,
    bit_depth: Option<u32>,
    sample_rate_hz: f64,
) -> (u8, u32, u64) {
    let sample_rate = if sample_rate_hz.is_finite() && sample_rate_hz > 0.0 {
        sample_rate_hz as u64
    } else {
        0
    };
    let depth = bit_depth.unwrap_or(0);
    if matches!(format, AudioFormat::Dsd) {
        return (4, depth, sample_rate);
    }
    let lossless = matches!(
        format,
        AudioFormat::Flac
            | AudioFormat::Alac
            | AudioFormat::Wav
            | AudioFormat::Aiff
            | AudioFormat::Ape
    );
    let tier = if lossless && (depth > 16 || sample_rate > 48_000) {
        3
    } else if lossless {
        2
    } else if !matches!(format, AudioFormat::Unknown) {
        1
    } else {
        0
    };
    (tier, depth, sample_rate)
}

/// Tier for the shared QualityBadge, mirroring its own derivation order
/// (MP3 first, then max / hires / cd) so a local card and a Qobuz card can
/// never disagree.
///
/// DSD is answered before the depth match for the reason `quality_qt::badge`
/// spells out: a DSD row arrives as depth 1 / rate 2 822 400, so `Some(_)`
/// caught it and every DSD file in the library wore the CD mark. The extra
/// `"max"` tier below is NOT folded into `hires` here — `LocalLibraryView
/// .qml:435` filters hi-res on the word and `QualityBadge.qml:66` folds it.
pub fn tier_of(format: &AudioFormat, bit_depth: Option<u32>, sample_rate_hz: f64) -> &'static str {
    if matches!(format, AudioFormat::Mp3) {
        return "mp3";
    }
    if matches!(format, AudioFormat::Dsd) {
        return "dsd";
    }
    let khz = if sample_rate_hz >= 1000.0 {
        sample_rate_hz / 1000.0
    } else {
        sample_rate_hz
    };
    match bit_depth {
        Some(b) if b >= 24 && khz > 96.0 => "max",
        Some(b) if b >= 24 => "hires",
        Some(_) => "cd",
        None if khz >= 44.1 => "cd",
        None => "",
    }
}

/// The detail line that rides under `tier_of`'s tier. Its ONLY job beyond
/// `home_qt::quality_detail_from_parts` is the DSD arm: that helper takes no
/// format, so it read a DSD row's nominal 1-bit depth and 2 822 400 Hz bit
/// rate literally and printed "1-bit / 2822.4 kHz" where the rest of the app
/// (and MyQBZ, via `quality_qt::badge`) prints `DSD64`. Every caller of
/// `tier_of` pairs with this so the two halves of one badge cannot disagree.
pub fn detail_of(format: &AudioFormat, bit_depth: Option<u32>, sample_rate_hz: f64) -> String {
    if matches!(format, AudioFormat::Dsd) {
        return crate::quality_qt::dsd_multiple_label(Some(sample_rate_hz));
    }
    crate::home_qt::quality_detail_from_parts(bit_depth, Some(sample_rate_hz))
}

/// Physical filter variants for one source-native album record. Source words
/// are projected through the same badge vocabulary as `AlbumRow.sources`, so
/// a scanned Qobuz purchase remains in the Offline bucket while preserving
/// its gold `qobuz_purchase` identity.
pub fn album_media_variants(album: &LocalAlbum) -> Vec<AlbumMediaVariant> {
    let quality_tier = tier_of(&album.format, album.bit_depth, album.sample_rate).to_string();
    let format = album.format.to_string();
    let source_words = if album.sources.is_empty() {
        vec![album.source.as_str()]
    } else {
        album.sources.iter().map(String::as_str).collect()
    };
    let mut variants = source_words
        .into_iter()
        .map(|source| {
            let raw = badge_source_raw(Some(source));
            AlbumMediaVariant {
                quality_tier: quality_tier.clone(),
                format: format.clone(),
                source: if raw.is_empty() {
                    badge_source(Some(source))
                } else {
                    raw
                },
            }
        })
        .collect::<Vec<_>>();
    variants.sort_by(|left, right| {
        left.source
            .cmp(&right.source)
            .then_with(|| left.quality_tier.cmp(&right.quality_tier))
            .then_with(|| left.format.cmp(&right.format))
    });
    variants.dedup();
    variants
}

pub fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// ---------------------------------------------------------------------------
// Artwork keys (id-keyed windowing: a cover can never land on the wrong row)
// ---------------------------------------------------------------------------

pub fn album_key(id: &str) -> String {
    format!("album:{id}")
}
/// THE disc's own display name, or empty when the box does not name its discs.
///
/// Shared by the album page's divider (`local_album_actions::disc_rows`) and
/// the ephemeral pane's block headers, because they are answering the same
/// question and two copies would drift.
///
/// WHY IT CANNOT JUST READ `album_group_title`: in folder grouping a box set
/// is deliberately ONE album, so that field is the group name with the disc
/// suffix STRIPPED (metadata.rs::strip_disc_suffix) — "Box (Disc 1)" and
/// "Box (Disc 2)" both collapse to "Box". That is what holds the box together
/// and it is also why every disc used to show the same heading.
///
/// Most trustworthy first:
///   1. the disc FOLDER's own titled tail ("Disc 01 - TV Series Soundtrack
///      #01" -> "TV Series Soundtrack #01"). This is where a box that names
///      its discs usually says so, and a tag cannot contradict it.
///   2. the raw `album` tag, when it names something the group title does not
///      already say. Compared AFTER stripping the disc suffix, so a tag of
///      "Box (Disc 2)" — which numbers rather than names — is rejected instead
///      of being echoed next to a "Disc 2" label.
///   3. empty, and the caller keeps whatever it drew before.
pub fn disc_display_title(t: &LocalTrack, group_title: &str) -> String {
    use qbz_library::MetadataExtractor;
    if let Some(from_folder) = std::path::Path::new(&t.file_path)
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .and_then(MetadataExtractor::disc_title_from_name)
    {
        return from_folder;
    }
    let tag = MetadataExtractor::strip_disc_suffix_public(&t.album);
    let group = MetadataExtractor::strip_disc_suffix_public(group_title);
    if tag.is_empty() || tag.eq_ignore_ascii_case(&group) {
        String::new()
    } else {
        tag
    }
}

/// THIS disc's own folder cover as an encoded `file://` url, or empty.
///
/// This direct URL remains the local-file fallback beside the track's id-keyed
/// `artwork_path`. Current scans preserve embedded/disc/collection precedence;
/// older database rows can still carry one album-root image for every disc.
///
/// Encoded through `artwork_qt::file_url`, which is not optional: these folder
/// names contain '#', and in a URL that opens a FRAGMENT — a raw concatenation
/// truncates the path and QML logs "Cannot open".
pub fn disc_cover_url(t: &LocalTrack) -> String {
    let Some(dir) = std::path::Path::new(&t.file_path).parent() else {
        return String::new();
    };
    qbz_library::MetadataExtractor::folder_artwork_in_dir(dir)
        .map(|p| crate::artwork_qt::file_url(&p))
        .unwrap_or_default()
}

/// Which layer of a source-owned artwork reference a consumer needs.
/// Album surfaces prefer the collection/box cover; track surfaces prefer the
/// item/disc cover. Both fall back to the other layer when their preferred one
/// is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtworkScope {
    Album,
    Track,
}

/// A portable artwork reference for a `LocalTrack`.
///
/// This is the single boundary between source-owned opaque tokens and Qt
/// surfaces that may no longer have the source row at render time (Recents,
/// MyQBZ, queue, search). Jellyfin/Subsonic tokens are source-qualified so the
/// shared artwork resolver never has to guess their owner; Plex keeps its
/// server-relative path; local files become encoded `file://` URLs. The value
/// is safe to persist because it contains no media-server credentials.
pub(crate) fn portable_artwork_ref(t: &LocalTrack, scope: ArtworkScope) -> Option<String> {
    let item = t.artwork_path.as_deref().filter(|value| !value.is_empty());
    let collection = t
        .collection_artwork_path
        .as_deref()
        .filter(|value| !value.is_empty());
    let token = match scope {
        ArtworkScope::Album => collection.or(item),
        ArtworkScope::Track => item.or(collection),
    }?;

    let source = t
        .source
        .as_deref()
        .and_then(SourceId::from_word)
        .unwrap_or(SourceId::LOCAL);
    if source == SourceId::JELLYFIN || source == SourceId::SUBSONIC {
        return Some(format!("{}:{token}", source.as_str()));
    }
    if source == SourceId::PLEX
        || token.starts_with("http://")
        || token.starts_with("https://")
        || token.starts_with("file://")
    {
        return Some(token.to_owned());
    }
    Some(crate::artwork_qt::file_url(token))
}

pub fn track_key(id: i64) -> String {
    format!("track:{id}")
}
pub fn folder_key(path: &str) -> String {
    format!("folder:{path}")
}
pub fn artist_key(name: &str) -> String {
    format!("artist:{name}")
}

/// The badge value for a raw `qbz_library` source column.
///
/// `qobuz_purchase` folds into `"offline"` DELIBERATELY and must keep doing
/// so: this value is what the Local Library's source chips filter on
/// (`LocalLibraryView.qml`'s `applyFilter`), and a purchased album is a Qobuz
/// download — giving it a fourth bucket would make it vanish the moment the
/// user ticks any source chip. The distinction the fold loses is restored
/// beside it by `badge_source_raw`, not by widening this function.
pub fn badge_source(raw: Option<&str>) -> String {
    match raw {
        Some("plex") => "plex".into(),
        // The media servers. Missing here they fell into the `_` arm and every
        // Jellyfin album in the grid drew the LOCAL HARD-DRIVE glyph — which is
        // not a cosmetic slip: the badge is what tells a user whose disk a
        // track lives on, and the source chips filter on this exact string.
        // Caught by looking at the window, 2026-08-20; no test and no audit
        // could see it, because folding to "local" is a perfectly valid answer
        // for a word this function was never taught.
        Some("jellyfin") => "jellyfin".into(),
        Some("subsonic") | Some("navidrome") | Some("gonic") | Some("airsonic")
        | Some("astiga") => "subsonic".into(),
        Some("qobuz_download") | Some("qobuz_purchase") => "offline".into(),
        _ => "local".into(),
    }
}

/// The raw source word the BADGE needs, or empty when the folded value above
/// already says everything.
///
/// Only `qobuz_purchase` qualifies today: it is the one raw value with a badge
/// of its own (`controls/SourceIcon.qml:75` draws the Qobuz mark in gold for
/// it) that `badge_source` destroys. `"user"` and `"qobuz_download"` are NOT
/// echoed here on purpose — they have no badge the folded value does not
/// already produce, and handing four QML consumers a second spelling of a
/// value they already handle is how the two drift apart.
///
/// Contract §9.4: this word is preserved on SCANNED LOCAL rows only. The
/// remote purchases feed (`library_qt::fetch_purchases`) keeps publishing
/// `"qobuz"` — it never consults the download registry, so stamping it there
/// would badge every purchase the user merely OWNS as locally downloaded.
pub fn badge_source_raw(raw: Option<&str>) -> String {
    match raw {
        Some("qobuz_purchase") => "qobuz_purchase".into(),
        _ => String::new(),
    }
}

/// The same badge vocabulary on Local Library cards, mixed feeds and pins.
/// Routing keeps its own source discriminator; these words are presentation.
pub(crate) fn source_badges<'a>(sources: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut badges = Vec::new();
    for source in sources {
        let raw = badge_source_raw(source);
        let badge = if raw.is_empty() {
            badge_source(source)
        } else {
            raw
        };
        if !badges.contains(&badge) {
            badges.push(badge);
        }
    }
    badges
}

pub(crate) fn album_badge_sources(album: &LocalAlbum) -> Vec<String> {
    let sources = if album.sources.is_empty() {
        std::slice::from_ref(&album.source)
    } else {
        album.sources.as_slice()
    };
    source_badges(sources.iter().map(|word| Some(word.as_str())))
}

/// `(owning source, raw token)` for a row's artwork — the pair the artwork
/// window needs in order to resolve it WITHOUT guessing.
///
/// This is the cheap half of what stage 4 set out to do, and the split matters.
/// Deciding WHOSE token this is costs one vocabulary lookup and happens here,
/// per row, while the row's provenance still exists — that is bug 3's fix.
/// Deciding what the token RESOLVES TO costs a credentials read for a remote
/// source, so it happens in `local_artwork::resolve_window_blocking`, over the
/// ~50 keys actually on screen rather than all 1703 in the document.
///
/// Resolving here instead was measured at 39-92 µs/row against 2 µs, on a
/// document rebuilt on every visit to Local Library. Same information, wrong
/// place.
pub fn art_token(source_word: Option<&str>, token: &str) -> Option<(SourceId, String)> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if let Some(path) = crate::remote_metadata_qt::local_art_path(token) {
        return Some((SourceId::LOCAL, path.to_string()));
    }
    // No word at all is the LOCAL case in practice (`local_tracks.source` is
    // empty for a plain scanned file, §3.1's vocabulary table).
    let id = SourceId::from_word(source_word.unwrap_or("")).unwrap_or(SourceId::LOCAL);
    Some((id, token.to_string()))
}

// ---------------------------------------------------------------------------
// Mappers (they also register the row's artwork REFERENCE in the art index —
// see `art_ref`: the row's own source decides what its token means)
// ---------------------------------------------------------------------------

pub fn map_album(a: LocalAlbum, art: &mut HashMap<String, (SourceId, String)>) -> AlbumRow {
    let names = [a.artist.as_str()]
        .into_iter()
        .chain(a.all_artists.split(','))
        .collect::<Vec<_>>();
    let aliases = crate::local_artist_match::build_artist_family_aliases(&names);
    let artists =
        crate::local_artist_match::album_credit_names(&a.artist, &a.all_artists, &aliases);
    map_album_with_artists(a, art, artists)
}

pub fn map_album_with_artists(
    a: LocalAlbum,
    art: &mut HashMap<String, (SourceId, String)>,
    artists: Vec<String>,
) -> AlbumRow {
    let media_variants = album_media_variants(&a);
    let key = album_key(&a.id);
    if let Some(p) = a.artwork_path.as_ref().filter(|p| !p.is_empty()) {
        let artwork_source = a.artwork_source.as_deref().unwrap_or(a.source.as_str());
        if let Some(t) = art_token(Some(artwork_source), p) {
            art.insert(key.clone(), t);
        }
    }
    let folder_count = a
        .source_folders
        .as_deref()
        .map(|s| s.split(',').filter(|x| !x.trim().is_empty()).count() as u32)
        .unwrap_or(0);
    let source = badge_source(Some(a.source.as_str()));
    let source_raw = badge_source_raw(Some(a.source.as_str()));
    let sources = album_badge_sources(&a);
    let favoriteable = album_favorite_source(&sources).is_some();
    let is_favorite = favoriteable && crate::library_qt::is_local_favorite("album", &a.id);
    AlbumRow {
        quality_tier: tier_of(&a.format, a.bit_depth, a.sample_rate).into(),
        quality_detail: detail_of(&a.format, a.bit_depth, a.sample_rate),
        format: a.format.to_string(),
        media_variants,
        genres: a.genres,
        year: a.year.map(|y| y.to_string()).unwrap_or_default(),
        duration: total_duration(a.total_duration_secs),
        track_count: a.track_count,
        art_key: key,
        source,
        sources,
        source_raw,
        directory_path: a.directory_path,
        all_artists: a.all_artists,
        artists,
        folder_count,
        is_favorite,
        favoriteable,
        id: a.id,
        title: a.title,
        artist: a.artist,
    }
}

/// Resolve the source word accepted by `local_favorites.db`. Prefer a real
/// local copy when a logical album spans several sources; otherwise retain a
/// server source so remote-only albums get the same heart affordance.
pub fn album_favorite_source(sources: &[String]) -> Option<&'static str> {
    if sources
        .iter()
        .any(|source| source.eq_ignore_ascii_case("local"))
    {
        Some("local")
    } else if sources
        .iter()
        .any(|source| source.eq_ignore_ascii_case("plex"))
    {
        Some("plex")
    } else if sources
        .iter()
        .any(|source| source.eq_ignore_ascii_case("jellyfin"))
    {
        Some("jellyfin")
    } else if sources.iter().any(|source| {
        matches!(
            source.to_ascii_lowercase().as_str(),
            "subsonic" | "navidrome" | "gonic" | "airsonic" | "astiga"
        )
    }) {
        Some("subsonic")
    } else {
        None
    }
}

/// Normalize every physical source carried by a native catalog album into
/// the exact words consumed by QML.  `AlbumRecord::source` is intentionally a
/// single routing choice; using only it made a mixed Plex/server card lose
/// the Plex favorite affordance depending on which copy won materialization.
pub fn catalog_album_sources(record: &AlbumRecord) -> Vec<String> {
    let fallback;
    let words: &[String] = if record.source_words.is_empty() {
        fallback = vec![if record.source_raw.trim().is_empty() {
            record.source.as_str().to_string()
        } else {
            record.source_raw.clone()
        }];
        &fallback
    } else {
        &record.source_words
    };
    let mut normalized = Vec::with_capacity(words.len());
    for word in words {
        let raw = badge_source_raw(Some(word));
        let value = if raw.is_empty() {
            badge_source(Some(word))
        } else {
            raw
        };
        if !normalized.iter().any(|known| known == &value) {
            normalized.push(value);
        }
    }
    normalized
}

pub fn map_track(t: &LocalTrack, art: &mut HashMap<String, (SourceId, String)>) -> TrackRow {
    let key = track_key(t.id);
    if let Some(p) = t
        .artwork_path
        .as_ref()
        .filter(|p| !p.is_empty())
        .or_else(|| t.collection_artwork_path.as_ref().filter(|p| !p.is_empty()))
    {
        if let Some(tok) = art_token(t.source.as_deref(), p) {
            art.insert(key.clone(), tok);
        }
    }
    TrackRow {
        id: t.id.to_string(),
        title: t.title.clone(),
        artist: t.artist.clone(),
        album: t.album_group_title.clone(),
        album_id: t.album_group_key.clone(),
        artist_id: String::new(),
        number: t.track_number.unwrap_or(0),
        disc: t.disc_number.unwrap_or(1),
        duration: mmss(t.duration_secs),
        quality_tier: tier_of(&t.format, t.bit_depth, t.sample_rate).into(),
        quality_detail: detail_of(&t.format, t.bit_depth, t.sample_rate),
        format: t.format.to_string(),
        genres: t.genres.clone(),
        year: t.year.map(|y| y.to_string()).unwrap_or_default(),
        art_key: key,
        art_path: String::new(),
        source: badge_source(t.source.as_deref()),
        source_raw: badge_source_raw(t.source.as_deref()),
        explicit: false,
        is_favorite: false,
    }
}

/// The bridge publishes strings, never structs.
pub fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::{album_favorite_source, portable_artwork_ref, ArtworkScope};
    use qbz_library::LocalTrack;

    fn sources(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn album_favorite_prefers_a_local_copy() {
        assert_eq!(
            album_favorite_source(&sources(&["plex", "jellyfin", "local"])),
            Some("local")
        );
    }

    #[test]
    fn album_favorite_accepts_plex_only() {
        assert_eq!(album_favorite_source(&sources(&["plex"])), Some("plex"));
    }

    #[test]
    fn album_favorite_accepts_media_servers_but_refuses_offline_only() {
        assert_eq!(
            album_favorite_source(&sources(&["jellyfin"])),
            Some("jellyfin")
        );
        assert_eq!(
            album_favorite_source(&sources(&["navidrome"])),
            Some("subsonic")
        );
        assert_eq!(
            album_favorite_source(&sources(&["qobuz_purchase", "offline"])),
            None
        );
    }

    #[test]
    fn album_badges_keep_physical_origins_without_changing_routing_words() {
        assert_eq!(
            super::source_badges([None, Some("user"), Some("local")]),
            ["local"]
        );
        assert_eq!(
            super::source_badges([
                Some("qobuz_download"),
                Some("qobuz_purchase"),
                Some("plex"),
                Some("jellyfin"),
                Some("navidrome"),
                Some("gonic")
            ]),
            ["offline", "qobuz_purchase", "plex", "jellyfin", "subsonic"]
        );
        // The menu/filter discriminator stays folded; only badge data keeps
        // the purchase identity. The card must not confuse a downloaded copy
        // with a catalog purchase that has never been downloaded.
        assert_eq!(super::badge_source(Some("qobuz_purchase")), "offline");
    }

    #[test]
    fn portable_artwork_ref_preserves_source_and_scope() {
        let jellyfin = LocalTrack {
            source: Some("jellyfin".into()),
            artwork_path: Some("track-id/disc-tag".into()),
            collection_artwork_path: Some("album-id/album-tag".into()),
            ..Default::default()
        };
        assert_eq!(
            portable_artwork_ref(&jellyfin, ArtworkScope::Track).as_deref(),
            Some("jellyfin:track-id/disc-tag")
        );
        assert_eq!(
            portable_artwork_ref(&jellyfin, ArtworkScope::Album).as_deref(),
            Some("jellyfin:album-id/album-tag")
        );

        let navidrome = LocalTrack {
            source: Some("navidrome".into()),
            collection_artwork_path: Some("al-123_hash".into()),
            ..Default::default()
        };
        assert_eq!(
            portable_artwork_ref(&navidrome, ArtworkScope::Album).as_deref(),
            Some("subsonic:al-123_hash")
        );
    }
}
