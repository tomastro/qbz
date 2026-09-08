//! M3U / M3U8.
//!
//! A line grammar, hand-rolled — no dependency. Every `m3u` crate on crates.io
//! is either HLS-focused or unmaintained, and the whole format is: comment
//! lines starting `#`, everything else a path or URL.
//!
//! Directives read: `#EXTM3U` (header), `#EXTINF:<secs>,<Artist - Title>`,
//! `#EXTALB:<album>`, `#EXTART:<artist>`, `#PLAYLIST:<name>`.

use super::decode::{file_stem, location_stem, split_artist_title, split_lines, track_or_skip};
use crate::errors::PlaylistImportError;
use crate::models::{ImportPlaylist, ImportProvider, ImportTrack};

/// The tags that make an `.m3u8` an HLS STREAM manifest rather than a playlist.
///
/// `.m3u8` is the extension for both, and they are not remotely the same file.
/// Without this guard a user who drops a stream manifest in gets a playlist of
/// segment filenames — garbage that then fails to match, with nothing telling
/// them why.
const HLS_MARKERS: &[&str] = &[
    "#EXT-X-STREAM-INF",
    "#EXT-X-TARGETDURATION",
    "#EXT-X-VERSION",
    "#EXT-X-MEDIA",
    "#EXT-X-PLAYLIST-TYPE",
];

pub(crate) fn looks_like_hls(text: &str) -> bool {
    let head: String = text.chars().take(4096).collect::<String>().to_uppercase();
    HLS_MARKERS.iter().any(|m| head.contains(m))
}

pub(crate) fn parse(text: &str, filename: &str) -> Result<ImportPlaylist, PlaylistImportError> {
    if looks_like_hls(text) {
        return Err(PlaylistImportError::HlsManifest);
    }

    let mut name = file_stem(filename);
    let mut tracks: Vec<ImportTrack> = Vec::new();

    // Pending directive state, consumed by the next non-comment line.
    let mut pending_title = String::new();
    let mut pending_artist = String::new();
    let mut pending_album: Option<String> = None;
    let mut pending_secs: Option<u64> = None;

    for line in split_lines(text) {
        if let Some(rest) = strip_tag(line, "#EXTINF:") {
            // "<seconds>,<text>" — seconds may be -1 (unknown) or fractional.
            let (secs, text) = match rest.split_once(',') {
                Some((s, t)) => (s.trim(), t.trim()),
                None => ("", rest.trim()),
            };
            pending_secs = parse_secs(secs);
            let (artist, title) = split_artist_title(text);
            // ONLY overwrite when the split actually found an artist. A
            // "#EXTART:Sigur Ros" followed by an #EXTINF with no " - " is a
            // real shape, and assigning the empty half unconditionally threw
            // the artist away — the row then went out title-only, which the
            // matcher tops out at 0.6 for and drops.
            if !artist.is_empty() {
                pending_artist = artist;
            }
            pending_title = title;
            continue;
        }
        if let Some(rest) = strip_tag(line, "#EXTALB:") {
            pending_album = Some(rest.trim().to_string()).filter(|s| !s.is_empty());
            continue;
        }
        if let Some(rest) = strip_tag(line, "#EXTART:") {
            // Only fills a gap — an "Artist - Title" #EXTINF already won.
            if pending_artist.is_empty() {
                pending_artist = rest.trim().to_string();
            }
            continue;
        }
        if let Some(rest) = strip_tag(line, "#PLAYLIST:") {
            let n = rest.trim();
            if !n.is_empty() {
                name = n.to_string();
            }
            continue;
        }
        if line.starts_with('#') {
            // Any other directive, including a bare #EXTM3U.
            continue;
        }

        // A media line. Its OWN stem is the title fallback — a bare-path M3U
        // carries nothing else.
        let title = if pending_title.is_empty() {
            location_stem(line)
        } else {
            std::mem::take(&mut pending_title)
        };
        let artist = std::mem::take(&mut pending_artist);
        let album = pending_album.take();
        let duration_ms = pending_secs.take().map(|s| s * 1000);
        if let Some(t) = track_or_skip(title, artist, album, duration_ms, None) {
            tracks.push(t);
        }
        pending_title.clear();
    }

    Ok(ImportPlaylist {
        provider: ImportProvider::File,
        provider_id: filename.to_string(),
        name,
        description: None,
        tracks,
    })
}

fn strip_tag<'a>(line: &'a str, tag: &str) -> Option<&'a str> {
    if line.len() >= tag.len() && line[..tag.len()].eq_ignore_ascii_case(tag) {
        Some(&line[tag.len()..])
    } else {
        None
    }
}

/// `#EXTINF` seconds. `-1` means unknown, and a fractional value is legal.
fn parse_secs(raw: &str) -> Option<u64> {
    let v: f64 = raw.trim().parse().ok()?;
    if v <= 0.0 {
        return None;
    }
    Some(v.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extinf_rows_carry_artist_title_and_duration() {
        let body = "#EXTM3U\n\
                    #EXTINF:245,Bob Dylan - Ballad of a Thin Man\n\
                    /music/dylan/thin_man.flac\n\
                    #EXTINF:-1,Unknown Length Song\n\
                    /music/x.flac\n";
        let p = parse(body, "/lists/Road Trip.m3u").unwrap();
        assert_eq!(p.name, "Road Trip");
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[0].artist, "Bob Dylan");
        assert_eq!(p.tracks[0].title, "Ballad of a Thin Man");
        // SECONDS x 1000 — the M3U field is seconds, unlike XSPF's.
        assert_eq!(p.tracks[0].duration_ms, Some(245_000));
        // -1 is "unknown", not a duration.
        assert_eq!(p.tracks[1].duration_ms, None);
        assert_eq!(p.tracks[1].artist, "");
        assert_eq!(p.tracks[1].title, "Unknown Length Song");
    }

    #[test]
    fn bare_paths_fall_back_to_the_file_stem() {
        let body = "/music/Sigur%20R%C3%B3s%20-%20Hoppipolla.flac\nC:\\m\\Track Two.mp3\n";
        let p = parse(body, "list.m3u").unwrap();
        assert_eq!(p.tracks.len(), 2);
        // The stem is the whole title; the " - " is NOT split here, because
        // a path stem is not the "Artist - Title" convention.
        assert_eq!(p.tracks[0].title, "Sigur Rós - Hoppipolla");
        assert_eq!(p.tracks[1].title, "Track Two");
    }

    #[test]
    fn extalb_extart_and_playlist_directives_are_read() {
        let body = "#EXTM3U\n\
                    #PLAYLIST:Summer 2026\n\
                    #EXTALB:Agaetis Byrjun\n\
                    #EXTART:Sigur Ros\n\
                    #EXTINF:300,Svefn-g-englar\n\
                    a.flac\n";
        let p = parse(body, "whatever.m3u").unwrap();
        assert_eq!(p.name, "Summer 2026");
        assert_eq!(p.tracks[0].album.as_deref(), Some("Agaetis Byrjun"));
        // No " - " in the EXTINF text, so #EXTART fills the artist.
        assert_eq!(p.tracks[0].artist, "Sigur Ros");
        assert_eq!(p.tracks[0].title, "Svefn-g-englar");
    }

    #[test]
    fn an_extinf_artist_beats_extart() {
        let body = "#EXTART:Wrong\n#EXTINF:100,Right - Song\na.flac\n";
        let p = parse(body, "x.m3u").unwrap();
        assert_eq!(p.tracks[0].artist, "Right");
    }

    #[test]
    fn crlf_and_lone_cr_both_parse() {
        let body = "#EXTM3U\r\n#EXTINF:10,A - B\r\na.flac\r#EXTINF:20,C - D\rb.flac\r";
        let p = parse(body, "x.m3u").unwrap();
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[1].title, "D");
    }

    #[test]
    fn an_hls_manifest_is_refused_not_parsed() {
        let body = "#EXTM3U\n\
                    #EXT-X-VERSION:3\n\
                    #EXT-X-TARGETDURATION:10\n\
                    #EXTINF:9.9,\n\
                    seg0.ts\n";
        assert!(matches!(
            parse(body, "stream.m3u8"),
            Err(PlaylistImportError::HlsManifest)
        ));
    }

    #[test]
    fn a_master_playlist_is_refused_too() {
        let body = "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1280000\nhigh.m3u8\n";
        assert!(matches!(
            parse(body, "master.m3u8"),
            Err(PlaylistImportError::HlsManifest)
        ));
    }
}
