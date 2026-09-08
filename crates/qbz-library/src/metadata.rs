//! Metadata extraction for audio files

use lofty::config::ParseOptions;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::thumbnails::{generate_thumbnail, generate_thumbnail_from_bytes};
use crate::{AudioFormat, AudioProperties, LibraryError, LocalTrack};

/// Metadata extractor using lofty
pub struct MetadataExtractor;

impl MetadataExtractor {
    fn normalize_field(value: Option<&str>) -> Option<String> {
        value
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    /// Iterate the file's tags with the primary tag FIRST, then the rest in
    /// file order. Backs the per-key cross-tag fallback below: old,
    /// repeatedly-retagged collections routinely hold the album /
    /// album-artist / date only in a SECONDARY tag (ID3v1/APE/Vorbis) while
    /// the file-type's primary tag lacks them — reading just the primary tag
    /// dropped them (#447 folder-name albums, #507 ignored Album Artist).
    fn tags_primary_first<'a>(
        tagged_file: &'a lofty::file::TaggedFile,
    ) -> impl Iterator<Item = &'a lofty::tag::Tag> + 'a {
        let primary = tagged_file.primary_tag();
        primary.into_iter().chain(
            tagged_file
                .tags()
                .iter()
                .filter(move |t| primary.map_or(true, |p| !std::ptr::eq(*t, p))),
        )
    }

    /// First non-empty string for `key` across all of the file's tags
    /// (primary first). When several tags disagree, the primary tag wins —
    /// deterministic, and matches what other players show.
    fn string_across_tags(tagged_file: &lofty::file::TaggedFile, key: &ItemKey) -> Option<String> {
        Self::string_from_tags(Self::tags_primary_first(tagged_file), key)
    }

    /// Every non-empty value for `key` across all tags, preserving primary-tag
    /// order and deduplicating case-insensitively. This is intentionally
    /// separate from the first-value fallback used by scalar metadata.
    fn strings_across_tags(tagged_file: &lofty::file::TaggedFile, key: &ItemKey) -> Vec<String> {
        let mut values = Vec::<String>::new();
        for tag in Self::tags_primary_first(tagged_file) {
            for value in tag.get_strings(key.clone()) {
                let value = value.trim();
                if !value.is_empty()
                    && !values
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(value))
                {
                    values.push(value.to_string());
                }
            }
        }
        values
    }

    /// Pure core of [`Self::string_across_tags`]: first non-empty, trimmed
    /// value for `key` yielded by the tag iterator (already in priority
    /// order). The empty check lives INSIDE the find_map so a blank value in
    /// one tag does not shadow a real value in a later tag.
    fn string_from_tags<'a>(
        mut tags: impl Iterator<Item = &'a lofty::tag::Tag>,
        key: &ItemKey,
    ) -> Option<String> {
        tags.find_map(|t| {
            t.get_string(key.clone())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
    }

    /// First track number across all tags (primary first).
    fn track_across_tags(tagged_file: &lofty::file::TaggedFile) -> Option<u32> {
        Self::track_from_tags(Self::tags_primary_first(tagged_file))
    }

    /// Pure core of [`Self::track_across_tags`].
    fn track_from_tags<'a>(mut tags: impl Iterator<Item = &'a lofty::tag::Tag>) -> Option<u32> {
        tags.find_map(|t| t.track())
    }

    /// First disc number across all tags (primary first).
    fn disk_across_tags(tagged_file: &lofty::file::TaggedFile) -> Option<u32> {
        Self::disk_from_tags(Self::tags_primary_first(tagged_file))
    }

    /// Pure core of [`Self::disk_across_tags`].
    fn disk_from_tags<'a>(mut tags: impl Iterator<Item = &'a lofty::tag::Tag>) -> Option<u32> {
        tags.find_map(|t| t.disk())
    }

    /// First parseable date's year across all tags (primary first);
    /// `Tag::date()` already falls back RecordingDate -> Year within a tag.
    fn year_across_tags(tagged_file: &lofty::file::TaggedFile) -> Option<u32> {
        Self::year_from_tags(Self::tags_primary_first(tagged_file))
    }

    /// Pure core of [`Self::year_across_tags`].
    fn year_from_tags<'a>(mut tags: impl Iterator<Item = &'a lofty::tag::Tag>) -> Option<u32> {
        tags.find_map(|t| t.date()).map(|ts| ts.year as u32)
    }

    fn strip_year_suffix(name: &str) -> String {
        let trimmed = name.trim();
        for (open, close) in [("(", ")"), ("[", "]")] {
            if trimmed.ends_with(close) {
                if let Some(start) = trimmed.rfind(open) {
                    let inside = &trimmed[start + 1..trimmed.len() - 1];
                    if inside.len() == 4 && inside.chars().all(|c| c.is_ascii_digit()) {
                        return trimmed[..start].trim().to_string();
                    }
                }
            }
        }
        trimmed.to_string()
    }

    fn strip_disc_suffix(title: &str) -> String {
        let trimmed = title.trim();

        for (open, close) in [("(", ")"), ("[", "]")] {
            if trimmed.ends_with(close) {
                if let Some(start) = trimmed.rfind(open) {
                    let inside = trimmed[start + 1..trimmed.len() - 1].trim();
                    if Self::is_disc_designator(inside) {
                        return trimmed[..start].trim().to_string();
                    }
                }
            }
        }

        let tokens: Vec<&str> = trimmed
            .split_whitespace()
            .filter(|token| *token != "-" && *token != "–" && *token != "—")
            .collect();

        if tokens.len() >= 2 {
            let last = tokens[tokens.len() - 1];
            let prev = tokens[tokens.len() - 2];
            if Self::is_disc_marker(prev) && last.chars().all(|c| c.is_ascii_digit()) {
                return tokens[..tokens.len() - 2].join(" ").trim().to_string();
            }
        }

        if let Some(last) = tokens.last() {
            if Self::is_disc_designator(last) {
                if tokens.len() > 1 {
                    return tokens[..tokens.len() - 1].join(" ").trim().to_string();
                }
            }
        }

        trimmed.to_string()
    }

    fn is_disc_marker(value: &str) -> bool {
        matches!(value.to_lowercase().as_str(), "disc" | "disk" | "cd")
    }

    fn is_disc_designator(value: &str) -> bool {
        let cleaned: String = value
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();

        if cleaned.starts_with("disc") {
            let rest = &cleaned[4..];
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }
        if cleaned.starts_with("disk") {
            let rest = &cleaned[4..];
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }
        if cleaned.starts_with("cd") {
            let rest = &cleaned[2..];
            return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        }

        false
    }

    fn is_disc_folder(name: &str) -> bool {
        Self::is_bare_disc_folder(name) || Self::is_titled_disc_folder(name)
    }

    /// `Disc 01 - TV Series Soundtrack #01`, `CD2 – Bonus Material`: a disc
    /// folder that also names the disc. Box sets are shipped this way, and
    /// without this arm every disc becomes its own album, because
    /// `album_root_dir` only climbs past folders this predicate accepts.
    ///
    /// Disjoint from the #147 false positives by construction. Those are album
    /// titles that END with a disc token (`Now 75 - CD1`,
    /// `100 Popular Classics, Disc 1`, `Relaxation Disc1`); here the
    /// designator must be the FIRST thing in the name. The separator must have
    /// whitespace on one side, so `CD-Rom Favourites` is not a disc folder
    /// either — an accidental merge of two albums is worse than the split this
    /// fixes, so the rule stays narrow.
    /// `strip_disc_suffix` for callers outside this crate — the disc-suffix
    /// rule is the one that decides whether a tag NAMES a disc or merely
    /// NUMBERS it, and a second copy of it elsewhere would drift.
    pub fn strip_disc_suffix_public(title: &str) -> String {
        Self::strip_disc_suffix(title)
    }

    /// The TITLE half of a titled disc folder — "Disc 1 - Rheingold" -> "Rheingold".
    ///
    /// `is_titled_disc_folder` already computes this split and throws the tail
    /// away; this returns it. PURELY ADDITIVE: nothing about grouping changes.
    /// In particular `strip_disc_suffix` and `album_root_dir` are untouched —
    /// they are what make a box set fold into ONE album, there is a regression
    /// test pinning that, and a per-disc LABEL must not be bought by breaking
    /// it.
    pub fn disc_title_from_name(name: &str) -> Option<String> {
        let trimmed = name.trim();
        let bytes = trimmed.as_bytes();

        let mut split = None;
        for (idx, ch) in trimmed.char_indices() {
            if !matches!(ch, '-' | '\u{2013}' | '\u{2014}' | ':') {
                continue;
            }
            let space_before = idx > 0 && bytes[idx - 1].is_ascii_whitespace();
            let after = idx + ch.len_utf8();
            let space_after = trimmed[after..].starts_with(char::is_whitespace);
            if space_before || space_after {
                split = Some((idx, after));
                break;
            }
        }
        let (head_end, tail_start) = split?;
        let head = trimmed[..head_end].trim();
        let tail = trimmed[tail_start..].trim();
        if tail.is_empty() || !Self::is_bare_disc_folder(head) {
            return None;
        }
        Some(tail.to_string())
    }

    fn is_titled_disc_folder(name: &str) -> bool {
        let trimmed = name.trim();
        let bytes = trimmed.as_bytes();

        let mut split = None;
        for (idx, ch) in trimmed.char_indices() {
            if !matches!(ch, '-' | '\u{2013}' | '\u{2014}' | ':') {
                continue;
            }
            let space_before = idx > 0 && bytes[idx - 1].is_ascii_whitespace();
            let after = idx + ch.len_utf8();
            let space_after = trimmed[after..].starts_with(char::is_whitespace);
            if space_before || space_after {
                split = Some((idx, after));
                break;
            }
        }

        let Some((head_end, tail_start)) = split else {
            return false;
        };

        let head = trimmed[..head_end].trim();
        let tail = trimmed[tail_start..].trim();

        // `Disc 1 -` with nothing after it is already the bare form.
        !tail.is_empty() && Self::is_bare_disc_folder(head)
    }

    fn is_bare_disc_folder(name: &str) -> bool {
        let lower = name.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();

        // Must contain at least one disc-related token
        let has_disc_token = tokens.iter().any(|token| {
            *token == "disc"
                || *token == "disk"
                || *token == "cd"
                || (token.starts_with("disc")
                    && token[4..].chars().all(|c| c.is_ascii_digit())
                    && !token[4..].is_empty())
                || (token.starts_with("disk")
                    && token[4..].chars().all(|c| c.is_ascii_digit())
                    && !token[4..].is_empty())
                || (token.starts_with("cd")
                    && token[2..].chars().all(|c| c.is_ascii_digit())
                    && !token[2..].is_empty())
        });

        if !has_disc_token {
            return false;
        }

        // A genuine disc folder name consists ONLY of disc-related tokens,
        // digits, and common modifiers like "bonus". If other words remain
        // after filtering these out, the name is an album title that happens
        // to contain "Disc 1" etc., not a standalone disc folder.
        // Examples that ARE disc folders: "Disc 1", "CD2", "Bonus Disc", "disc01"
        // Examples that are NOT: "Relaxation Disc1", "Now 75 - CD1",
        //   "100 Popular Classics, Disc 1"
        let has_extra_words = tokens.iter().any(|token| {
            // Pure digits are fine
            if token.chars().all(|c| c.is_ascii_digit()) {
                return false;
            }
            // Disc keywords are fine
            if *token == "disc" || *token == "disk" || *token == "cd" {
                return false;
            }
            // Disc+number compounds are fine (disc1, cd02, etc.)
            for prefix in &["disc", "disk", "cd"] {
                if token.starts_with(prefix) {
                    let rest = &token[prefix.len()..];
                    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                        return false;
                    }
                }
            }
            // Common disc folder modifiers are fine
            if matches!(*token, "bonus" | "extra" | "side" | "part") {
                return false;
            }
            // Anything else means this is not a pure disc folder
            true
        });

        !has_extra_words
    }

    fn disc_number_from_name(name: &str) -> Option<u32> {
        let lower = name.to_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();

        for (i, token) in tokens.iter().enumerate() {
            if (*token == "disc" || *token == "disk" || *token == "cd")
                && tokens
                    .get(i + 1)
                    .map_or(false, |t| t.chars().all(|c| c.is_ascii_digit()))
            {
                if let Some(next) = tokens.get(i + 1) {
                    if let Ok(value) = next.parse::<u32>() {
                        if value > 0 {
                            return Some(value);
                        }
                    }
                }
            }

            for prefix in ["disc", "disk", "cd"] {
                if token.starts_with(prefix) {
                    let rest = &token[prefix.len()..];
                    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                        if let Ok(value) = rest.parse::<u32>() {
                            if value > 0 {
                                return Some(value);
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Try to extract a track number from the filename.
    /// Handles common patterns like:
    /// - "01 - Title.flac"
    /// - "01. Title.flac"
    /// - "01_Title.flac"
    /// - "Track 01.flac"
    /// - "1-01 Title.flac" (disc-track)
    pub fn infer_track_number_from_filename(file_path: &Path) -> Option<u32> {
        let stem = file_path.file_stem()?.to_str()?;
        let trimmed = stem.trim();

        // Pattern: starts with digits
        if let Some(cap) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
            // Collect leading digits
            let digit_end = 1 + cap.chars().take_while(|c| c.is_ascii_digit()).count();
            let num_str = &trimmed[..digit_end];
            let rest = &trimmed[digit_end..];

            // Check for disc-track pattern FIRST: "D-TT" like "1-01 Title", "2-05 Song"
            // Only when leading number is 1-2 digits (disc number) followed by dash+digits
            if digit_end <= 2 && rest.starts_with('-') {
                let after_dash = &rest[1..];
                let track_digits: String = after_dash
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if !track_digits.is_empty() {
                    if let Ok(n) = track_digits.parse::<u32>() {
                        if n > 0 && n < 10000 {
                            return Some(n);
                        }
                    }
                }
            }

            // Regular pattern: digits followed by separator
            // "01 - Title", "01. Title", "01_Title", "01-Title"
            let rest_trimmed = rest.trim_start();
            let has_separator = rest_trimmed.starts_with('-')
                || rest_trimmed.starts_with('.')
                || rest_trimmed.starts_with('_')
                || rest_trimmed.starts_with(' ')
                || rest_trimmed.is_empty();

            if has_separator {
                if let Ok(n) = num_str.parse::<u32>() {
                    if n > 0 && n < 10000 {
                        return Some(n);
                    }
                }
            }
        }

        // Pattern: "Track 01" or "Track01"
        let lower = trimmed.to_lowercase();
        if lower.starts_with("track") {
            let after = trimmed[5..].trim_start();
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                if let Ok(n) = digits.parse::<u32>() {
                    if n > 0 && n < 10000 {
                        return Some(n);
                    }
                }
            }
        }

        None
    }

    pub fn infer_disc_number(file_path: &Path) -> Option<u32> {
        let parent_dir = file_path.parent()?;
        let parent_name = parent_dir.file_name()?.to_str()?;
        if !Self::is_disc_folder(parent_name) {
            return None;
        }
        Self::disc_number_from_name(parent_name)
    }

    /// Returns true if the folder name looks like an audio encoding/quality
    /// directory (e.g., "FLAC 24-bit - 96 kHz", "MP3 320 kbps").
    fn is_encoding_folder(name: &str) -> bool {
        let lower = name.to_lowercase();
        let first_word = lower
            .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .find(|tok| !tok.is_empty());

        if let Some(word) = first_word {
            if matches!(
                word,
                "flac"
                    | "mp3"
                    | "aac"
                    | "alac"
                    | "wav"
                    | "aiff"
                    | "ogg"
                    | "dsd"
                    | "opus"
                    | "wma"
                    | "ape"
                    | "pcm"
            ) {
                return true;
            }
        }

        // Standalone bitrate patterns like "320kbps"
        if lower.contains("kbps") {
            return true;
        }

        false
    }

    fn album_root_dir(file_path: &Path) -> Option<PathBuf> {
        let mut dir = file_path.parent()?.to_path_buf();

        // Skip past disc and encoding subdirectories to find the actual album root.
        // Handles: album/track, album/disc1/track, album/FLAC 24-96/track,
        //          album/FLAC 24-96/disc1/track
        for _ in 0..2 {
            let name = dir.file_name().and_then(|s| s.to_str());
            match name {
                Some(n) if Self::is_disc_folder(n) || Self::is_encoding_folder(n) => {
                    dir = dir.parent()?.to_path_buf();
                }
                _ => break,
            }
        }

        Some(dir)
    }

    fn infer_artist_album(
        file_path: &Path,
        library_roots: &[PathBuf],
    ) -> (Option<String>, Option<String>) {
        let album_dir = Self::album_root_dir(file_path);
        let album_name = album_dir
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(Self::strip_year_suffix);

        let artist_name = album_dir
            .as_ref()
            .and_then(|p| p.parent())
            .and_then(|parent| {
                // Root clamp (spec 2026-07-19-local-album-grouping-mode §C):
                // an album dir hanging DIRECTLY off a library root means the
                // "parent folder" IS the root itself — its name ("Music", …)
                // is structural, never an artist. Untagged albums at root
                // level (e.g. a tagless DSD set) used to surface with the
                // root's name as the artist.
                if library_roots.iter().any(|root| root == parent) {
                    None
                } else {
                    parent
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(Self::strip_year_suffix)
                }
            });

        if artist_name.is_none() {
            if let Some(album_dir_name) = album_name.as_deref() {
                if let Some((artist, album)) = album_dir_name.split_once(" - ") {
                    return (
                        Some(Self::strip_year_suffix(artist)),
                        Some(Self::strip_year_suffix(album)),
                    );
                }
            }
        }

        (artist_name, album_name)
    }

    pub fn album_group_info(file_path: &Path, tag_album: Option<&str>) -> (String, String) {
        let album_dir = Self::album_root_dir(file_path);
        let group_key = album_dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().to_string())
            .unwrap_or_else(|| file_path.to_string_lossy().to_string());

        let mut group_title = tag_album
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .filter(|value| !value.eq_ignore_ascii_case("Unknown Album"))
            .map(|value| value.to_string())
            .or_else(|| {
                album_dir
                    .as_ref()
                    .and_then(|dir| dir.file_name())
                    .and_then(|s| s.to_str())
                    .map(Self::strip_year_suffix)
            })
            .unwrap_or_else(|| "Unknown Album".to_string());

        group_title = Self::strip_disc_suffix(&group_title);

        (group_key, group_title)
    }

    /// Extract metadata from an audio file
    pub fn extract(file_path: &Path) -> Result<LocalTrack, LibraryError> {
        Self::extract_with_roots(file_path, &[])
    }

    /// Like [`Self::extract`], but the caller supplies the library roots the
    /// file is being scanned under. Roots feed the untagged-artist root
    /// clamp (see `infer_artist_album`); the plain `extract` passes none, so
    /// ephemeral / single-file extraction keeps the legacy inference.
    pub fn extract_with_roots(
        file_path: &Path,
        library_roots: &[PathBuf],
    ) -> Result<LocalTrack, LibraryError> {
        log::debug!("Extracting audio metadata");

        // DSD containers aren't lofty-readable: qbz-dsd demuxes them (tech
        // props + embedded ID3v2 for DSF; trailing ID3 for DFF when present).
        if qbz_dsd::is_dsd_path(file_path) {
            return Self::extract_dsd(file_path, library_roots);
        }

        // Probe the file — WITHOUT pulling the embedded cover art.
        //
        // MEASURED 2026-08-22 on a 247-file box set on a NAS: reading tags
        // took 9.27 s of a 9.30 s open, while everything else — artwork
        // caching, ids, grouping, sorting — took 29 ms. And parallelising the
        // reads across 8 threads had bought only 1.6x, which is the signature
        // of a bandwidth wall rather than a latency one.
        //
        // A FLAC's cover art lives INSIDE the tag, so `read()` was pulling
        // multiple megabytes of picture data per file across the network to
        // recover a title and a track number. The pictures are not wanted
        // here: this function fills a row, and the cover is resolved
        // separately by `extract_artwork` / `find_folder_artwork` for ONE file
        // per album (13 of 247 in that box).
        //
        // `read_properties` stays on — duration, bit depth and sample rate
        // come from the header and are cheap.
        let tagged_file = Probe::open(file_path)
            .map_err(|e| LibraryError::Metadata(format!("Failed to open file: {}", e)))?
            .options(ParseOptions::new().read_cover_art(false))
            .read()
            .map_err(|e| LibraryError::Metadata(format!("Failed to read file: {}", e)))?;

        // Get the primary tag (prefer ID3v2/Vorbis/APE)
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag());

        // Get audio properties
        let properties = tagged_file.properties();
        let duration_secs = properties.duration().as_secs();
        let sample_rate = properties.sample_rate().unwrap_or(44100) as f64;
        let bit_depth = properties.bit_depth().map(|b| b as u32);
        let channels = properties.channels().unwrap_or(2) as u8;

        // Get file metadata
        let file_metadata = fs::metadata(file_path).map_err(LibraryError::Io)?;
        let file_size_bytes = file_metadata.len();
        let last_modified = file_metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Detect format
        let format = Self::detect_format(file_path);

        // Get filename for fallback title
        let filename = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();

        let (fallback_artist, fallback_album) = Self::infer_artist_album(file_path, library_roots);

        let inferred_disc = Self::infer_disc_number(file_path);

        // Build track
        let track = if tag.is_some() {
            // Per-key fallback across ALL of the file's tags (primary first):
            // old, repeatedly-retagged collections routinely carry the album /
            // album-artist / date only in a secondary tag (ID3v1/APE/Vorbis)
            // while the primary tag lacks them. Reading just the primary tag
            // dropped them — the album column then got the folder-derived
            // backfill and masqueraded as metadata (folder-name albums,
            // folder-level grouping, MIN(year) across mixed folders = #447),
            // and album_artist landed NULL (Various Artists everywhere = #507).
            let album_tag = Self::string_across_tags(&tagged_file, &ItemKey::AlbumTitle);
            if album_tag.is_none() {
                // Diagnostic (#447): the folder backfill fires for this file —
                // the signal when a user reports folder-name albums on
                // "tagged" files.
                log::debug!(
                    "[library] no album tag in any tag of {}; using folder-derived name",
                    file_path.display()
                );
            }
            let album_title = album_tag
                .or_else(|| fallback_album.clone())
                .unwrap_or_else(|| "Unknown Album".to_string());
            let (album_group_key, album_group_title) =
                Self::album_group_info(file_path, Some(album_title.as_str()));
            let genres = Self::strings_across_tags(&tagged_file, &ItemKey::Genre);

            LocalTrack {
                id: 0,
                file_path: file_path.to_string_lossy().to_string(),
                title: Self::string_across_tags(&tagged_file, &ItemKey::TrackTitle)
                    .unwrap_or(filename),
                artist: Self::string_across_tags(&tagged_file, &ItemKey::TrackArtist)
                    .or_else(|| fallback_artist.clone())
                    .unwrap_or_else(|| "Unknown Artist".to_string()),
                album: album_title,
                album_artist: Self::string_across_tags(&tagged_file, &ItemKey::AlbumArtist),
                album_group_key,
                album_group_title,
                track_number: Self::track_across_tags(&tagged_file)
                    .or_else(|| Self::infer_track_number_from_filename(file_path)),
                disc_number: Self::disk_across_tags(&tagged_file)
                    .and_then(|d| if d > 0 { Some(d) } else { None })
                    .or(inferred_disc),
                year: Self::year_across_tags(&tagged_file),
                genre: genres.first().cloned(),
                genres,
                catalog_number: Self::string_across_tags(&tagged_file, &ItemKey::CatalogNumber),
                duration_secs,
                format,
                bit_depth,
                sample_rate,
                channels,
                file_size_bytes,
                cue_file_path: None,
                cue_start_secs: None,
                cue_end_secs: None,
                artwork_path: None,
                collection_artwork_path: None,
                last_modified,
                indexed_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                source: None,
                qobuz_track_id: None,
                // Identity tags as Picard writes them (lofty maps
                // MUSICBRAINZ_TRACKID -> MusicBrainzRecordingId and
                // MUSICBRAINZ_RELEASETRACKID -> MusicBrainzTrackId; ISRC/TSRC
                // -> Isrc). Same cross-tag fallback as every other field.
                isrc: Self::string_across_tags(&tagged_file, &ItemKey::Isrc),
                musicbrainz_recording_id: Self::string_across_tags(
                    &tagged_file,
                    &ItemKey::MusicBrainzRecordingId,
                ),
                musicbrainz_track_id: Self::string_across_tags(
                    &tagged_file,
                    &ItemKey::MusicBrainzTrackId,
                ),
                musicbrainz_release_id: Self::string_across_tags(
                    &tagged_file,
                    &ItemKey::MusicBrainzReleaseId,
                ),
                musicbrainz_release_group_id: Self::string_across_tags(
                    &tagged_file,
                    &ItemKey::MusicBrainzReleaseGroupId,
                ),
                musicbrainz_artist_id: Self::string_across_tags(
                    &tagged_file,
                    &ItemKey::MusicBrainzArtistId,
                ),
                is_network_mount: false,
            }
        } else {
            // No tag found, use defaults
            let album_title = fallback_album
                .clone()
                .unwrap_or_else(|| "Unknown Album".to_string());
            let (album_group_key, album_group_title) =
                Self::album_group_info(file_path, Some(album_title.as_str()));

            LocalTrack {
                id: 0,
                file_path: file_path.to_string_lossy().to_string(),
                title: filename,
                artist: fallback_artist.unwrap_or_else(|| "Unknown Artist".to_string()),
                album: album_title,
                album_artist: None,
                album_group_key,
                album_group_title,
                track_number: Self::infer_track_number_from_filename(file_path),
                disc_number: inferred_disc,
                year: None,
                genre: None,
                genres: Vec::new(),
                catalog_number: None,
                duration_secs,
                format,
                bit_depth,
                sample_rate,
                channels,
                file_size_bytes,
                cue_file_path: None,
                cue_start_secs: None,
                cue_end_secs: None,
                artwork_path: None,
                collection_artwork_path: None,
                last_modified,
                indexed_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                source: None,
                qobuz_track_id: None,
                isrc: None,
                musicbrainz_recording_id: None,
                musicbrainz_track_id: None,
                musicbrainz_release_id: None,
                musicbrainz_release_group_id: None,
                musicbrainz_artist_id: None,
                is_network_mount: false,
            }
        };

        Ok(track)
    }

    /// Build a LocalTrack from a DSF/DFF file via qbz-dsd. Tag-read failures
    /// degrade to filename-derived metadata — a DSD file must still index.
    fn extract_dsd(
        file_path: &Path,
        library_roots: &[PathBuf],
    ) -> Result<LocalTrack, LibraryError> {
        let demux = qbz_dsd::open_dsd(file_path)
            .map_err(|e| LibraryError::Metadata(format!("Failed to read DSD file: {}", e)))?;
        let info = demux.info().clone();
        drop(demux);

        let file_metadata = fs::metadata(file_path).map_err(LibraryError::Io)?;
        let file_size_bytes = file_metadata.len();
        let last_modified = file_metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let filename = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
        let (fallback_artist, fallback_album) = Self::infer_artist_album(file_path, library_roots);
        let inferred_disc = Self::infer_disc_number(file_path);

        let tags = &info.tags;
        let album_title = Self::normalize_field(tags.album.as_deref())
            .or_else(|| fallback_album.clone())
            .unwrap_or_else(|| "Unknown Album".to_string());
        let (album_group_key, album_group_title) =
            Self::album_group_info(file_path, Some(album_title.as_str()));
        let genres = tags.genre.iter().cloned().collect();

        Ok(LocalTrack {
            id: 0,
            file_path: file_path.to_string_lossy().to_string(),
            title: tags.title.clone().unwrap_or(filename),
            artist: Self::normalize_field(tags.artist.as_deref())
                .or_else(|| fallback_artist.clone())
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            album: album_title,
            album_artist: tags.album_artist.clone(),
            album_group_key,
            album_group_title,
            track_number: tags
                .track_number
                .or_else(|| Self::infer_track_number_from_filename(file_path)),
            disc_number: tags.disc_number.filter(|d| *d > 0).or(inferred_disc),
            year: tags.year.and_then(|y| u32::try_from(y).ok()),
            genre: tags.genre.clone(),
            genres,
            catalog_number: None,
            duration_secs: info.duration_secs(),
            format: AudioFormat::Dsd,
            // 1-bit stream; sample_rate carries the DSD bit rate (2 822 400 =
            // DSD64) — the badge layer derives "DSD64/128/256" from it.
            bit_depth: Some(1),
            sample_rate: info.dsd_rate as f64,
            channels: info.channels as u8,
            file_size_bytes,
            cue_file_path: None,
            cue_start_secs: None,
            cue_end_secs: None,
            artwork_path: None,
            collection_artwork_path: None,
            last_modified,
            indexed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            source: None,
            qobuz_track_id: None,
            isrc: None,
            musicbrainz_recording_id: None,
            musicbrainz_track_id: None,
            musicbrainz_release_id: None,
            musicbrainz_release_group_id: None,
            musicbrainz_artist_id: None,
            is_network_mount: false,
        })
    }

    /// Extract audio properties without full metadata
    pub fn extract_properties(file_path: &Path) -> Result<AudioProperties, LibraryError> {
        if qbz_dsd::is_dsd_path(file_path) {
            let demux = qbz_dsd::open_dsd(file_path)
                .map_err(|e| LibraryError::Metadata(format!("Failed to read DSD file: {}", e)))?;
            let info = demux.info();
            return Ok(AudioProperties {
                duration_secs: info.duration_secs(),
                bit_depth: Some(1),
                sample_rate: info.dsd_rate as f64,
                channels: info.channels as u8,
            });
        }

        // Properties only — the embedded cover is megabytes of pure waste on
        // this path (see the note in `extract_with_roots`).
        let tagged_file = Probe::open(file_path)
            .map_err(|e| LibraryError::Metadata(format!("Failed to open file: {}", e)))?
            .options(ParseOptions::new().read_cover_art(false))
            .read()
            .map_err(|e| LibraryError::Metadata(format!("Failed to read file: {}", e)))?;

        let properties = tagged_file.properties();

        Ok(AudioProperties {
            duration_secs: properties.duration().as_secs(),
            bit_depth: properties.bit_depth().map(|b| b as u32),
            sample_rate: properties.sample_rate().unwrap_or(44100) as f64,
            channels: properties.channels().unwrap_or(2) as u8,
        })
    }

    /// Determine AudioFormat from file extension
    pub fn detect_format(path: &Path) -> AudioFormat {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .as_deref()
        {
            Some("flac") => AudioFormat::Flac,
            Some("m4a") => AudioFormat::Alac,
            Some("wav") => AudioFormat::Wav,
            Some("aiff") | Some("aif") => AudioFormat::Aiff,
            Some("ape") => AudioFormat::Ape,
            Some("mp3") => AudioFormat::Mp3,
            Some("dsf") | Some("dff") => AudioFormat::Dsd,
            _ => AudioFormat::Unknown,
        }
    }

    /// Extract and save artwork as thumbnail to cache directory
    pub fn extract_artwork(file_path: &Path, _cache_dir: &Path) -> Option<String> {
        if qbz_dsd::is_dsd_path(file_path) {
            let demux = qbz_dsd::open_dsd(file_path).ok()?;
            let art = demux.info().tags.artwork.clone()?;
            let cache_key = file_path.to_string_lossy().to_string();
            return match generate_thumbnail_from_bytes(&art, &cache_key) {
                Ok(thumbnail_path) => Some(thumbnail_path.to_string_lossy().to_string()),
                Err(e) => {
                    log::warn!("Failed to generate DSD thumbnail: {}", e);
                    None
                }
            };
        }

        let tagged_file = Probe::open(file_path).ok()?.read().ok()?;
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())?;

        let picture = tag.pictures().first()?;

        let cache_key = file_path.to_string_lossy().to_string();

        match generate_thumbnail_from_bytes(picture.data(), &cache_key) {
            Ok(thumbnail_path) => Some(thumbnail_path.to_string_lossy().to_string()),
            Err(e) => {
                log::warn!("Failed to generate embedded-art thumbnail: {}", e);
                None
            }
        }
    }

    /// The RAW embedded picture bytes of an audio file (first picture, the
    /// same pick `extract_artwork` makes), WITHOUT the 500px thumbnail
    /// encode. This is the re-open-the-source seam for the on-demand large
    /// art tier (contract `2026-08-15-immersive-completion` 04 §5): the DB
    /// keeps only the 500px thumb path, so a big slot re-derives its bounded
    /// 1600px JPEG from these bytes. DSD containers are opened through
    /// `qbz_dsd` exactly like `extract_artwork`.
    pub fn extract_artwork_bytes(file_path: &Path) -> Option<Vec<u8>> {
        if qbz_dsd::is_dsd_path(file_path) {
            let demux = qbz_dsd::open_dsd(file_path).ok()?;
            return demux.info().tags.artwork.clone();
        }

        let tagged_file = Probe::open(file_path).ok()?.read().ok()?;
        let tag = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())?;

        tag.pictures().first().map(|p| p.data().to_vec())
    }

    /// Generate thumbnail from an existing artwork file
    pub fn cache_artwork_file(artwork_path: &Path, _cache_dir: &Path) -> Option<String> {
        if !artwork_path.is_file() {
            return None;
        }

        match generate_thumbnail(artwork_path) {
            Ok(thumbnail_path) => Some(thumbnail_path.to_string_lossy().to_string()),
            Err(e) => {
                log::warn!("Failed to generate folder-art thumbnail: {}", e);
                None
            }
        }
    }

    /// Generate a stable thumbnail for artwork bytes whose source may be a
    /// temporary editor staging file. The content hash is part of the cache
    /// key, so replacing an album cover cannot reuse the thumbnail generated
    /// for the previous image at the same audio or folder path.
    pub fn cache_artwork_bytes(bytes: &[u8]) -> Option<String> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let cache_key = format!("metadata-editor-artwork-{:016x}", hasher.finish());
        match generate_thumbnail_from_bytes(bytes, &cache_key) {
            Ok(thumbnail_path) => Some(thumbnail_path.to_string_lossy().to_string()),
            Err(error) => {
                log::warn!("Failed to generate edited-art thumbnail: {}", error);
                None
            }
        }
    }

    /// Look for folder artwork by file name heuristics
    /// A cover living in THIS directory, ignoring the album root.
    ///
    /// `find_folder_artwork` deliberately PREFERS the album root — it gives
    /// index 0 a `dir_bonus` of 5 and compares with a strict `>`, so
    /// `Box/cover.jpg` (105) beats `Box/Disc 01 - Name/cover.jpg` (100) on
    /// every track of every disc. That is right for the album's own cover and
    /// exactly wrong for a per-DISC one: it is why a box set whose discs each
    /// ship their own art still resolved to one shared image on all of them.
    ///
    /// So this is the un-biased sibling, used only where the caller already
    /// knows it is asking about one disc's folder. It does NOT touch the scan
    /// path and changes no stored value.
    pub fn folder_artwork_in_dir(dir: &Path) -> Option<String> {
        let mut best: Option<(u32, PathBuf)> = None;
        let mut candidates = Vec::new();
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default();
            if !Self::is_supported_artwork_ext(&ext) {
                continue;
            }
            candidates.push(path.clone());
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            // Same vocabulary the scanner scores with, simplified: an exact
            // well-known name wins and a name containing one is a fallback.
            const EXACT: &[&str] = &["cover", "folder", "front", "album", "artwork", "art"];
            let score = if EXACT.contains(&stem.as_str()) {
                2
            } else if EXACT.iter().any(|k| stem.contains(k)) {
                1
            } else {
                continue;
            };
            if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                best = Some((score, path));
            }
        }
        best.map(|(_, p)| p.to_string_lossy().to_string())
            .or_else(|| {
                // A single image in a disc directory is unambiguous even when
                // it is named after the movement rather than `cover.jpg`.
                (candidates.len() == 1)
                    .then(|| candidates[0].to_string_lossy().to_string())
            })
    }

    pub fn find_folder_artwork(
        audio_file_path: &Path,
        album_title: Option<&str>,
    ) -> Option<String> {
        let parent_dir = audio_file_path.parent()?;
        let album_dir =
            Self::album_root_dir(audio_file_path).unwrap_or_else(|| parent_dir.to_path_buf());

        let mut dirs_to_check: Vec<PathBuf> = Vec::new();
        if album_dir != parent_dir {
            dirs_to_check.push(album_dir.clone());
        }
        dirs_to_check.push(parent_dir.to_path_buf());

        let album_key = album_title
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .filter(|value| !value.eq_ignore_ascii_case("Unknown Album"))
            .map(Self::strip_disc_suffix)
            .and_then(|value| Self::normalize_artwork_key(&value));
        let folder_key = album_dir
            .file_name()
            .and_then(|s| s.to_str())
            .map(Self::strip_disc_suffix)
            .and_then(|value| Self::normalize_artwork_key(&value));

        let mut best: Option<(PathBuf, i32)> = None;
        let mut best_score = 0;
        let mut candidate_count = 0;

        for (index, dir) in dirs_to_check.iter().enumerate() {
            let dir_bonus = if index == 0 { 5 } else { 0 };
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !Self::is_supported_artwork_ext(&ext) {
                    continue;
                }

                candidate_count += 1;
                let file_stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .trim();
                let file_key = match Self::normalize_artwork_key(file_stem) {
                    Some(key) => key,
                    None => {
                        let fallback = file_stem.to_lowercase();
                        if fallback.trim().is_empty() {
                            continue;
                        }
                        fallback
                    }
                };

                let mut score =
                    Self::artwork_score(&file_key, album_key.as_deref(), folder_key.as_deref());
                if score == 0 {
                    score = 5;
                }
                score += dir_bonus;

                if score > best_score {
                    best_score = score;
                    best = Some((path, score));
                }
            }
        }

        if let Some((path, score)) = best {
            if score >= 10 || candidate_count == 1 {
                return Some(path.to_string_lossy().to_string());
            }
        }

        None
    }

    fn normalize_artwork_key(value: &str) -> Option<String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        let normalized: String = trimmed
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        if normalized.is_empty() {
            None
        } else {
            Some(normalized)
        }
    }

    fn is_supported_artwork_ext(ext: &str) -> bool {
        matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")
    }

    fn artwork_score(file_key: &str, album_key: Option<&str>, folder_key: Option<&str>) -> i32 {
        const EXACT: &[&str] = &["cover", "folder", "front", "album", "artwork", "art"];
        let mut score = 0;

        if EXACT.iter().any(|name| *name == file_key) {
            score = score.max(100);
        }
        if let Some(key) = album_key {
            if file_key == key {
                score = score.max(95);
            } else if file_key.contains(key) {
                score = score.max(70);
            }
        }
        if let Some(key) = folder_key {
            if file_key == key {
                score = score.max(90);
            } else if file_key.contains(key) {
                score = score.max(65);
            }
        }
        if EXACT.iter().any(|name| file_key.contains(name)) {
            score = score.max(80);
        }

        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format() {
        assert_eq!(
            MetadataExtractor::detect_format(Path::new("test.flac")),
            AudioFormat::Flac
        );
        assert_eq!(
            MetadataExtractor::detect_format(Path::new("test.m4a")),
            AudioFormat::Alac
        );
        assert_eq!(
            MetadataExtractor::detect_format(Path::new("test.wav")),
            AudioFormat::Wav
        );
        assert_eq!(
            MetadataExtractor::detect_format(Path::new("test.mp3")),
            AudioFormat::Mp3
        );
    }

    #[test]
    fn test_is_encoding_folder() {
        // QBZ-generated quality folder names
        assert!(MetadataExtractor::is_encoding_folder(
            "FLAC 16-bit - 44.1 kHz"
        ));
        assert!(MetadataExtractor::is_encoding_folder(
            "FLAC 24-bit - 96 kHz"
        ));
        assert!(MetadataExtractor::is_encoding_folder(
            "FLAC 24-bit - 192 kHz"
        ));
        assert!(MetadataExtractor::is_encoding_folder("MP3 320 kbps"));

        // Common encoding folder names from other tools
        assert!(MetadataExtractor::is_encoding_folder("FLAC"));
        assert!(MetadataExtractor::is_encoding_folder("flac"));
        assert!(MetadataExtractor::is_encoding_folder("MP3"));
        assert!(MetadataExtractor::is_encoding_folder("WAV"));
        assert!(MetadataExtractor::is_encoding_folder("ALAC"));
        assert!(MetadataExtractor::is_encoding_folder("DSD"));
        assert!(MetadataExtractor::is_encoding_folder("320kbps"));

        // Not encoding folders
        assert!(!MetadataExtractor::is_encoding_folder("Abbey Road"));
        assert!(!MetadataExtractor::is_encoding_folder("Disc 1"));
        assert!(!MetadataExtractor::is_encoding_folder("The Beatles"));
        assert!(!MetadataExtractor::is_encoding_folder("2024"));
    }

    #[test]
    fn test_album_root_dir_plain() {
        // artist/album/track.flac -> album/
        let path = Path::new("/music/EELS/Beautiful Freak/01 - Novocaine.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/EELS/Beautiful Freak"));
    }

    #[test]
    fn test_album_root_dir_disc_folder() {
        // artist/album/disc1/track.flac -> album/
        let path = Path::new("/music/EELS/Beautiful Freak/Disc 1/01 - Novocaine.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/EELS/Beautiful Freak"));
    }

    #[test]
    fn test_album_root_dir_encoding_folder() {
        // artist/album/quality/track.flac -> album/
        let path =
            Path::new("/music/EELS/Beautiful Freak/FLAC 24-bit - 96 kHz/01 - Novocaine.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/EELS/Beautiful Freak"));
    }

    #[test]
    fn test_album_root_dir_encoding_and_disc() {
        // artist/album/quality/disc1/track.flac -> album/
        let path = Path::new(
            "/music/EELS/Beautiful Freak/FLAC 24-bit - 96 kHz/Disc 1/01 - Novocaine.flac",
        );
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/EELS/Beautiful Freak"));
    }

    #[test]
    fn test_infer_artist_album_root_clamp() {
        let roots = vec![PathBuf::from("/music")];

        // Album dir directly under the library root: the root's own name
        // must NOT become the artist (the untagged DSD-at-root case,
        // spec 2026-07-19-local-album-grouping-mode §C).
        let path = Path::new("/music/Some DSD Album/01 - Track.dsf");
        let (artist, album) = MetadataExtractor::infer_artist_album(path, &roots);
        assert_eq!(artist, None);
        assert_eq!(album.as_deref(), Some("Some DSD Album"));

        // Same, behind a disc folder (the clamp looks at the album ROOT dir).
        let path = Path::new("/music/Some DSD Album/Disc 1/01 - Track.dsf");
        let (artist, album) = MetadataExtractor::infer_artist_album(path, &roots);
        assert_eq!(artist, None);
        assert_eq!(album.as_deref(), Some("Some DSD Album"));

        // The "Artist - Album" split still kicks in when the parent-folder
        // inference is clamped away.
        let path = Path::new("/music/MAKE-UP - Saint Seiya Best/01.dsf");
        let (artist, album) = MetadataExtractor::infer_artist_album(path, &roots);
        assert_eq!(artist.as_deref(), Some("MAKE-UP"));
        assert_eq!(album.as_deref(), Some("Saint Seiya Best"));

        // Normal Artist/Album layout is NOT clamped (parent != root).
        let path = Path::new("/music/EELS/Beautiful Freak/01.flac");
        let (artist, album) = MetadataExtractor::infer_artist_album(path, &roots);
        assert_eq!(artist.as_deref(), Some("EELS"));
        assert_eq!(album.as_deref(), Some("Beautiful Freak"));

        // A REAL artist folder whose name matches the root's name is also
        // not clamped (its parent is the root's artist folder, not the root).
        let roots = vec![PathBuf::from("/media/Music")];
        let path = Path::new("/media/Music/Music/Some Album/01.flac");
        let (artist, _) = MetadataExtractor::infer_artist_album(path, &roots);
        assert_eq!(artist.as_deref(), Some("Music"));

        // No roots passed (ephemeral / single-file legacy path): inference
        // is unchanged — parent folder name wins as before.
        let path = Path::new("/music/Some DSD Album/01 - Track.dsf");
        let (artist, _) = MetadataExtractor::infer_artist_album(path, &[]);
        assert_eq!(artist.as_deref(), Some("music"));
    }

    #[test]
    fn test_infer_track_number_from_filename() {
        // Common patterns: "01 - Title"
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new(
                "/music/01 - Novocaine.flac"
            )),
            Some(1)
        );
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new(
                "/music/12 - Beautiful Freak.flac"
            )),
            Some(12)
        );
        // "01. Title"
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new(
                "/music/03. Song Name.flac"
            )),
            Some(3)
        );
        // "01_Title"
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new("/music/05_Track.flac")),
            Some(5)
        );
        // Just number
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new("/music/07.flac")),
            Some(7)
        );
        // "Track 01"
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new("/music/Track 09.flac")),
            Some(9)
        );
        // "1-01 Title" (disc-track)
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new("/music/2-05 Song.flac")),
            Some(5)
        );
        // Not a track number (title starting with non-track digits)
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new("/music/Novocaine.flac")),
            None
        );
        // Zero is not a valid track number
        assert_eq!(
            MetadataExtractor::infer_track_number_from_filename(Path::new(
                "/music/00 - Intro.flac"
            )),
            None
        );
    }

    #[test]
    fn test_is_disc_folder_true() {
        // Pure disc folders
        assert!(MetadataExtractor::is_disc_folder("Disc 1"));
        assert!(MetadataExtractor::is_disc_folder("disc 2"));
        assert!(MetadataExtractor::is_disc_folder("Disc1"));
        assert!(MetadataExtractor::is_disc_folder("disc01"));
        assert!(MetadataExtractor::is_disc_folder("CD 1"));
        assert!(MetadataExtractor::is_disc_folder("CD1"));
        assert!(MetadataExtractor::is_disc_folder("cd2"));
        assert!(MetadataExtractor::is_disc_folder("Disk 3"));
        assert!(MetadataExtractor::is_disc_folder("Bonus Disc"));
        assert!(MetadataExtractor::is_disc_folder("Bonus Disc 1"));
        assert!(MetadataExtractor::is_disc_folder("Extra CD 2"));
        assert!(MetadataExtractor::is_disc_folder("Side Disc 1"));
    }

    #[test]
    fn test_is_disc_folder_false_album_names() {
        // Album names containing disc/cd keywords — NOT disc folders (issue #147)
        assert!(!MetadataExtractor::is_disc_folder(
            "100 Popular Classics, Disc 1"
        ));
        assert!(!MetadataExtractor::is_disc_folder(
            "100 Popular Classics_ Best Loved Works of the Great Composers, Disc 1"
        ));
        assert!(!MetadataExtractor::is_disc_folder("Relaxation Disc1"));
        assert!(!MetadataExtractor::is_disc_folder("Now 75 - CD1"));
        assert!(!MetadataExtractor::is_disc_folder(
            "Match of the Day - The Album CD1"
        ));
        assert!(!MetadataExtractor::is_disc_folder("20 Blues Greats"));
        assert!(!MetadataExtractor::is_disc_folder("The Beatles"));
        assert!(!MetadataExtractor::is_disc_folder("Abbey Road"));
    }

    #[test]
    fn test_is_disc_folder_titled() {
        // Box sets name the disc after the designator. Every one of these used
        // to become its own album, in the DEFAULT (folder) grouping mode.
        assert!(MetadataExtractor::is_disc_folder(
            "Disc 01 - TV Series Soundtrack #01"
        ));
        assert!(MetadataExtractor::is_disc_folder("CD1 - Bonus Material"));
        assert!(MetadataExtractor::is_disc_folder("Disc 2 \u{2013} Live"));
        assert!(MetadataExtractor::is_disc_folder("Disc 3: The Remixes"));
        assert!(MetadataExtractor::is_disc_folder(
            "Bonus Disc - Rarities 1994-1999"
        ));
        assert!(MetadataExtractor::is_disc_folder("cd 04 - instrumentals"));
    }

    #[test]
    fn test_is_disc_folder_titled_stays_narrow() {
        // The designator must LEAD. These are the issue #147 shapes with a
        // separator in them, and they must stay album titles.
        assert!(!MetadataExtractor::is_disc_folder("Now 75 - CD1"));
        assert!(!MetadataExtractor::is_disc_folder(
            "Match of the Day - The Album CD1"
        ));
        assert!(!MetadataExtractor::is_disc_folder(
            "Greatest Hits - Disc 1 of 2 Collection"
        ));
        // No whitespace beside the separator: a hyphenated word, not a title.
        assert!(!MetadataExtractor::is_disc_folder("CD-Rom Favourites"));
        // A leading disc word with no separator at all.
        assert!(!MetadataExtractor::is_disc_folder("Disc Jockey Anthems"));
        // Nothing after the separator is the bare form, not the titled one.
        assert!(!MetadataExtractor::is_titled_disc_folder("Disc 1 -"));
    }

    #[test]
    fn test_album_root_dir_titled_disc_folders_group_as_one() {
        // A 13-disc box set: every disc directory must resolve to the SAME
        // album root, or the library shows 13 albums.
        let box_root = Path::new("/music/Seiji Yokoyama/Saint Seiya Eternal CD-Box");
        let first = MetadataExtractor::album_root_dir(Path::new(
            "/music/Seiji Yokoyama/Saint Seiya Eternal CD-Box/Disc 01 - TV Series Soundtrack #01/01. Pegasus Fantasy.flac",
        ))
        .unwrap();
        let last = MetadataExtractor::album_root_dir(Path::new(
            "/music/Seiji Yokoyama/Saint Seiya Eternal CD-Box/Disc 13 - Complete Song Collection 3/07. Blue Forever.flac",
        ))
        .unwrap();
        assert_eq!(first, box_root);
        assert_eq!(last, box_root);
    }

    #[test]
    fn test_disc_number_recovered_from_titled_disc_folder() {
        assert_eq!(
            MetadataExtractor::disc_number_from_name("Disc 01 - TV Series Soundtrack #01"),
            Some(1)
        );
        assert_eq!(
            MetadataExtractor::disc_number_from_name("CD13 - Complete Song Collection 3"),
            Some(13)
        );
    }

    #[test]
    fn test_album_root_dir_album_with_disc_in_name() {
        // Issue #147: album names containing "Disc N" should NOT be treated as disc folders
        let path = Path::new("/music/Various Artists/100 Popular Classics, Disc 1/01 - Track.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(
            root,
            Path::new("/music/Various Artists/100 Popular Classics, Disc 1")
        );

        let path = Path::new("/music/Various Artists/Relaxation Disc1/01 - Track.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/Various Artists/Relaxation Disc1"));

        let path = Path::new("/music/Various Artists/Now 75 - CD1/01 - Track.flac");
        let root = MetadataExtractor::album_root_dir(path).unwrap();
        assert_eq!(root, Path::new("/music/Various Artists/Now 75 - CD1"));
    }

    // ---- Cross-tag fallback (#447/#507) ----------------------------------
    //
    // Old, repeatedly-retagged collections routinely carry the album /
    // album-artist / date only in a SECONDARY tag (ID3v1/APE/Vorbis) while
    // the file-type's primary tag lacks them. These tests build in-memory
    // tags (no audio files needed) and exercise the pure fallback cores
    // used by `extract`.

    #[test]
    fn cross_tag_album_read_falls_back_to_secondary_tag() {
        // #447: the primary tag (ID3v2) has no album; it lives only in ID3v1.
        let mut primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        primary.insert_text(ItemKey::TrackTitle, "Song".to_string());
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v1);
        secondary.insert_text(ItemKey::AlbumTitle, "ALBUM.".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::string_from_tags(tags.iter().copied(), &ItemKey::AlbumTitle)
                .as_deref(),
            Some("ALBUM.")
        );
    }

    #[test]
    fn cross_tag_album_artist_read_falls_back_to_secondary_tag() {
        // #507: the album artist exists only in the APE tag.
        let primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Ape);
        secondary.insert_text(ItemKey::AlbumArtist, "Curated Artist".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::string_from_tags(tags.iter().copied(), &ItemKey::AlbumArtist)
                .as_deref(),
            Some("Curated Artist")
        );
    }

    #[test]
    fn cross_tag_read_prefers_first_tag_on_conflict() {
        // The primary tag comes first in the iterator, so its value wins —
        // deterministic conflict policy (matches other players).
        let mut primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        primary.insert_text(ItemKey::AlbumTitle, "Primary Album".to_string());
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Ape);
        secondary.insert_text(ItemKey::AlbumTitle, "Other Album".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::string_from_tags(tags.iter().copied(), &ItemKey::AlbumTitle)
                .as_deref(),
            Some("Primary Album")
        );
    }

    #[test]
    fn cross_tag_read_skips_empty_values() {
        let mut primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        primary.insert_text(ItemKey::AlbumTitle, "   ".to_string());
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Ape);
        secondary.insert_text(ItemKey::AlbumTitle, "Real Album".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::string_from_tags(tags.iter().copied(), &ItemKey::AlbumTitle)
                .as_deref(),
            Some("Real Album")
        );
    }

    #[test]
    fn cross_tag_year_read_falls_back_to_secondary_tag() {
        // #447 year: the date exists only in the secondary tag.
        let primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Ape);
        secondary.insert_text(ItemKey::RecordingDate, "2025".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::year_from_tags(tags.iter().copied()),
            Some(2025)
        );
    }

    #[test]
    fn cross_tag_track_and_disc_read_fall_back_to_secondary_tag() {
        let primary = lofty::tag::Tag::new(lofty::tag::TagType::Id3v2);
        let mut secondary = lofty::tag::Tag::new(lofty::tag::TagType::Ape);
        secondary.insert_text(ItemKey::TrackNumber, "7".to_string());
        secondary.insert_text(ItemKey::DiscNumber, "2".to_string());

        let tags = [&primary, &secondary];
        assert_eq!(
            MetadataExtractor::track_from_tags(tags.iter().copied()),
            Some(7)
        );
        assert_eq!(
            MetadataExtractor::disk_from_tags(tags.iter().copied()),
            Some(2)
        );
    }
}
