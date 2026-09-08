//! PLS — an INI whose keys carry the row index as a suffix.
//!
//! ```ini
//! [playlist]
//! File1=/music/a.flac
//! Title1=Bob Dylan - Ballad of a Thin Man
//! Length1=245
//! NumberOfEntries=1
//! ```
//!
//! `NumberOfEntries` is DELIBERATELY IGNORED as an iteration bound: real files
//! disagree with their own header often enough (hand edits, truncated writes)
//! that trusting it drops rows that are right there. The rows discovered are
//! the rows imported.

use std::collections::BTreeMap;

use super::decode::{file_stem, location_stem, split_artist_title, split_lines, track_or_skip};
use crate::errors::PlaylistImportError;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

#[derive(Default)]
struct Row {
    file: Option<String>,
    title: Option<String>,
    length: Option<i64>,
}

pub(crate) fn parse(text: &str, filename: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    // BTreeMap so the rows come out in index order regardless of file order —
    // a hand-edited PLS can easily interleave them.
    let mut rows: BTreeMap<u32, Row> = BTreeMap::new();

    for line in split_lines(text) {
        if line.starts_with('[') || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        let Some((prefix, idx)) = split_indexed_key(key) else {
            continue;
        };
        let row = rows.entry(idx).or_default();
        // LAST WINS on a duplicate key — the INI convention, and the only
        // reading under which a hand-appended correction takes effect.
        match prefix.as_str() {
            "file" => row.file = Some(value.to_string()),
            "title" => row.title = Some(value.to_string()),
            "length" => row.length = value.parse::<i64>().ok(),
            _ => {}
        }
    }

    let mut tracks: Vec<ImportTrack> = Vec::new();
    for row in rows.into_values() {
        let (artist, title) = match row.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => split_artist_title(t),
            // No TitleN: the FileN stem is all there is.
            None => (
                String::new(),
                row.file.as_deref().map(location_stem).unwrap_or_default(),
            ),
        };
        // `-1` (and 0) mean unknown. Seconds -> ms.
        let duration_ms = row
            .length
            .filter(|l| *l > 0)
            .map(|l| l as u64 * 1000);
        if let Some(t) = track_or_skip(title, artist, None, duration_ms, None) {
            tracks.push(t);
        }
    }

    Ok(ImportPlaylist {
        provider: ImportProvider::File,
        provider_id: filename.to_string(),
        name: file_stem(filename),
        description: None,
        tracks,
    })
}

/// `"Title12"` -> `("title", 12)`. Case-insensitive, because PLS writers
/// disagree about it (`FILE1`, `File1`, `file1` all occur).
fn split_indexed_key(key: &str) -> Option<(String, u32)> {
    let digits_start = key.len() - key.chars().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits_start == key.len() || digits_start == 0 {
        return None;
    }
    let idx: u32 = key[digits_start..].parse().ok()?;
    Some((key[..digits_start].trim().to_ascii_lowercase(), idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[playlist]\n\
                          File1=/music/dylan/thin_man.flac\n\
                          Title1=Bob Dylan - Ballad of a Thin Man\n\
                          Length1=245\n\
                          File2=/music/x/Second%20Song.flac\n\
                          Length2=-1\n\
                          NumberOfEntries=2\n\
                          Version=2\n";

    #[test]
    fn numbered_keys_become_rows_in_index_order() {
        let p = parse(SAMPLE, "/lists/Party.pls").unwrap();
        assert_eq!(p.name, "Party");
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].artist, "Bob Dylan");
        assert_eq!(p.tracks[0].title, "Ballad of a Thin Man");
        assert_eq!(p.tracks[0].duration_ms, Some(245_000));
        // No TitleN -> the FileN stem, percent-decoded.
        assert_eq!(p.tracks[1].title, "Second Song");
        assert_eq!(p.tracks[1].duration_ms, None);
    }

    #[test]
    fn rows_out_of_order_still_come_out_in_index_order() {
        let body = "[playlist]\nTitle2=B - Two\nFile2=b.flac\nTitle1=A - One\nFile1=a.flac\n";
        let p = parse(body, "x.pls").unwrap();
        assert_eq!(p.tracks[0].title, "One");
        assert_eq!(p.tracks[1].title, "Two");
    }

    #[test]
    fn keys_are_case_insensitive_and_duplicates_take_the_last() {
        let body = "[PLAYLIST]\nFILE1=a.flac\ntitle1=First - Wrong\nTitle1=First - Right\n";
        let p = parse(body, "x.pls").unwrap();
        assert_eq!(p.tracks[0].title, "Right");
    }

    #[test]
    fn a_missing_header_still_parses() {
        // Half the PLS files in the wild have no [playlist] line.
        let body = "File1=a.flac\nTitle1=A - B\n";
        let p = parse(body, "x.pls").unwrap();
        assert_eq!(p.tracks.len(), 1);
    }

    #[test]
    fn number_of_entries_does_not_bound_the_rows() {
        // The header lies; the rows win.
        let body = "[playlist]\nNumberOfEntries=1\nFile1=a.flac\nTitle1=A - 1\n\
                    File2=b.flac\nTitle2=B - 2\nFile3=c.flac\nTitle3=C - 3\n";
        let p = parse(body, "x.pls").unwrap();
        assert_eq!(p.tracks.len(), 3);
    }

    #[test]
    fn a_row_with_neither_title_nor_file_is_dropped() {
        let body = "[playlist]\nLength1=100\nFile2=b.flac\nTitle2=B - Two\n";
        let p = parse(body, "x.pls").unwrap();
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tracks[0].title, "Two");
    }
}
