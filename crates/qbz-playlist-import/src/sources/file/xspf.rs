//! XSPF — the XML playlist format.
//!
//! `roxmltree`, not `quick-xml`: XSPF files are tiny (the byte wall forecloses
//! the streaming case that would justify an event state machine), and a
//! read-only DOM walked by LOCAL element name is thirty lines instead of a
//! parser with modes. Both crates were already in the lock, so neither costs a
//! new download.
//!
//! Namespace-lenient by construction: every lookup is `tag_name().name()`,
//! which is the local part. Files written with `xmlns="http://xspf.org/ns/0/"`
//! and files written without it walk identically — and both exist in the wild.

use super::decode::{is_isrc_shaped, location_stem, track_or_skip};
use crate::errors::PlaylistImportError;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

pub(crate) fn parse(text: &str, filename: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    let doc = roxmltree::Document::parse(text)
        .map_err(|e| PlaylistImportError::Parse(format!("XSPF: {e}")))?;
    let root = doc.root_element();

    let name = child_text(root, "title")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| super::decode::file_stem(filename));
    let description = child_text(root, "annotation").filter(|s| !s.is_empty());

    let mut tracks: Vec<ImportTrack> = Vec::new();
    // `<trackList>` may be nested under the root or (malformed but common)
    // appear at any depth; descendants covers both without a special case.
    for track in root
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "track")
    {
        let location = child_text(track, "location").unwrap_or_default();
        let title = child_text(track, "title")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| location_stem(&location));
        // `<creator>` is a CLEAN artist field — never run the "Artist - Title"
        // splitter on it. XSPF is the one file format that carries the two
        // separately, which is exactly why its rows match well.
        let artist = child_text(track, "creator").unwrap_or_default();
        let album = child_text(track, "album");
        // *** <duration> IS ALREADY MILLISECONDS. No x1000. ***
        // This is the single most common porting error in this format: M3U and
        // PLS are seconds and XSPF is not, so a copied line multiplies a
        // correct value by a thousand and every duration bonus turns into a
        // duration penalty.
        let duration_ms = child_text(track, "duration")
            .and_then(|d| d.trim().parse::<u64>().ok())
            .filter(|d| *d > 0);
        let isrc = find_isrc(track);

        if let Some(t) = track_or_skip(title, artist, album, duration_ms, isrc) {
            tracks.push(t);
        }
    }

    Ok(ImportPlaylist {
        provider: ImportProvider::File,
        provider_id: filename.to_string(),
        name,
        description,
        tracks,
    })
}

fn child_text<'a>(node: roxmltree::Node<'a, 'a>, name: &str) -> Option<String> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
}

/// Sweep the track's subtree for an ISRC.
///
/// XSPF has no ISRC element; taggers put it inside `<extension>` under wildly
/// different vendor element names, so the only portable rule is "any text node
/// under this track that IS an ISRC". Shape-checked, never guessed.
///
/// `<identifier>` is EXCLUDED ON PURPOSE: it is where MusicBrainz recording ids
/// live, and a MBID landing in `isrc` would fire the matcher's score-1.0
/// short-circuit against a completely unrelated Qobuz track. The shape check
/// already rejects a UUID, and the exclusion means we never even ask.
fn find_isrc(track: roxmltree::Node) -> Option<String> {
    for n in track.descendants() {
        if !n.is_element() {
            continue;
        }
        if n.tag_name().name() == "identifier" {
            continue;
        }
        // An `isrc`-named element wins outright when its text is shaped right.
        let text = n.text().map(str::trim).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let cleaned: String = text
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if is_isrc_shaped(&cleaned) {
            return Some(cleaned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMESPACED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<playlist version="1" xmlns="http://xspf.org/ns/0/">
  <title>Road Trip</title>
  <annotation>Songs for the drive</annotation>
  <trackList>
    <track>
      <location>file:///music/Sigur%20R%C3%B3s.flac</location>
      <title>Hoppipolla</title>
      <creator>Sigur Ros</creator>
      <album>Takk...</album>
      <duration>268000</duration>
    </track>
  </trackList>
</playlist>"#;

    #[test]
    fn namespaced_documents_walk_by_local_name() {
        let p = parse(NAMESPACED, "ignored.xspf").unwrap();
        assert_eq!(p.name, "Road Trip");
        assert_eq!(p.description.as_deref(), Some("Songs for the drive"));
        assert_eq!(p.tracks.len(), 1);
        assert_eq!(p.tracks[0].title, "Hoppipolla");
        // <creator> is a clean artist — NOT run through the dash splitter.
        assert_eq!(p.tracks[0].artist, "Sigur Ros");
        assert_eq!(p.tracks[0].album.as_deref(), Some("Takk..."));
    }

    #[test]
    fn duration_is_milliseconds_and_is_not_multiplied() {
        let p = parse(NAMESPACED, "x.xspf").unwrap();
        // 268000 ms = 4:28. If this ever reads 268_000_000 the x1000 crept in.
        assert_eq!(p.tracks[0].duration_ms, Some(268_000));
    }

    #[test]
    fn a_document_without_the_namespace_parses_identically() {
        let body = r#"<playlist version="1"><trackList><track>
            <title>Song</title><creator>Band</creator><duration>1000</duration>
        </track></trackList></playlist>"#;
        let p = parse(body, "no-ns.xspf").unwrap();
        assert_eq!(p.tracks[0].title, "Song");
        assert_eq!(p.tracks[0].duration_ms, Some(1000));
    }

    #[test]
    fn a_missing_title_falls_back_to_the_location_stem() {
        let body = r#"<playlist><trackList><track>
            <location>file:///m/Track%20Nine.flac</location>
        </track></trackList></playlist>"#;
        let p = parse(body, "x.xspf").unwrap();
        assert_eq!(p.tracks[0].title, "Track Nine");
    }

    #[test]
    fn a_missing_playlist_title_falls_back_to_the_filename() {
        let body = r#"<playlist><trackList><track><title>A</title></track></trackList></playlist>"#;
        let p = parse(body, "/lists/My Mix.xspf").unwrap();
        assert_eq!(p.name, "My Mix");
    }

    #[test]
    fn an_isrc_in_an_extension_is_found_and_normalized() {
        // A vendor extension with its namespace properly DECLARED — an
        // undeclared prefix is invalid XML and roxmltree is right to refuse it.
        let body = r#"<playlist xmlns:v="urn:x-vendor"><trackList><track>
            <title>A</title>
            <extension application="urn:x"><v:isrc>us-ko1-16-00123</v:isrc></extension>
        </track></trackList></playlist>"#;
        let p = parse(body, "x.xspf").unwrap();
        assert_eq!(p.tracks[0].isrc.as_deref(), Some("USKO11600123"));
    }

    #[test]
    fn a_plain_isrc_element_in_an_extension_is_found_too() {
        let body = r#"<playlist><trackList><track>
            <title>A</title>
            <extension application="urn:x"><isrc>USKO11600123</isrc></extension>
        </track></trackList></playlist>"#;
        let p = parse(body, "x.xspf").unwrap();
        assert_eq!(p.tracks[0].isrc.as_deref(), Some("USKO11600123"));
    }

    #[test]
    fn an_identifier_mbid_never_becomes_an_isrc() {
        let body = r#"<playlist><trackList><track>
            <title>A</title>
            <identifier>https://musicbrainz.org/recording/b1a9c0e9-1b47-4f3d-9a1e-000000000000</identifier>
        </track></trackList></playlist>"#;
        let p = parse(body, "x.xspf").unwrap();
        assert_eq!(p.tracks[0].isrc, None);
    }

    #[test]
    fn malformed_xml_is_a_parse_error_not_a_panic() {
        assert!(matches!(
            parse("<playlist><track>", "x.xspf"),
            Err(PlaylistImportError::Parse(_))
        ));
    }
}
