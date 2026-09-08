//! Playlist FILES — XSPF / PLS / M3U / M3U8.
//!
//! Filesystem-free by design (ADR-006): the app reads bytes through `rfd` and
//! passes them with the filename. The crate never opens a path, which is what
//! keeps it testable with `&[u8]` literals and correct inside a Flatpak, where
//! the portal — not the app — owns file access.
//!
//! # ONLY THE TRACK LIST IS READ
//!
//! The paths inside these files are never opened, never copied and never added
//! to the Local Library. Each entry becomes a title/artist/album/duration
//! probe and is matched against the Qobuz catalog like any other import. This
//! is a product promise, not an implementation detail — the modal says it in
//! so many words, and no code here may quietly grow a file read.

pub(crate) mod decode;
mod m3u;
mod pls;
mod xspf;

use super::{guard_size, FileFormat};
use crate::errors::PlaylistImportError;
use crate::models::ImportPlaylist;

/// Parse a playlist file.
///
/// Empty results are an ERROR, not an empty playlist: a file that produced no
/// tracks means the user picked the wrong thing, or the format is subtly not
/// what it claimed, and "imported 0 tracks" is a worse answer than saying so.
pub fn parse(
    format: FileFormat,
    bytes: &[u8],
    filename: &str,
) -> Result<ImportPlaylist, PlaylistImportError> {
    guard_size(bytes)?;
    let text = decode::decode_text(bytes);
    let playlist = match format {
        FileFormat::M3u => m3u::parse(&text, filename)?,
        FileFormat::Pls => pls::parse(&text, filename)?,
        FileFormat::Xspf => xspf::parse(&text, filename)?,
    };
    if playlist.tracks.is_empty() {
        return Err(PlaylistImportError::EmptyPlaylist);
    }
    Ok(playlist)
}

/// Decide the format from the bytes, with the extension as a hint only.
///
/// THE SNIFF WINS. Extensions are renamed, mailed, downloaded through services
/// that rewrite them and typed by hand; the first few bytes of an XML document
/// are not. The extension breaks ties the content cannot — specifically, a
/// bare-path M3U with no `#EXTM3U` header is indistinguishable from a text file
/// on content alone, so there the extension is the whole signal.
pub fn detect_format(bytes: &[u8], filename: &str) -> Result<FileFormat, PlaylistImportError> {
    guard_size(bytes)?;
    let text = decode::decode_text(bytes);
    let head: String = text.chars().take(4096).collect();
    let trimmed = head.trim_start();
    let lower = trimmed.to_ascii_lowercase();

    // XML first: unambiguous, and cheap to be sure about.
    if trimmed.starts_with('<') && lower.contains("<playlist") {
        return Ok(FileFormat::Xspf);
    }
    // PLS: the header, or (headerless, which is common) a FileN key.
    if lower.starts_with("[playlist]") || has_pls_key(&lower) {
        return Ok(FileFormat::Pls);
    }
    // Extended M3U. The HLS guard runs in the PARSER, not here: detection's
    // job is "which parser", and refusing a stream manifest is a parse-time
    // answer the caller can localize.
    if lower.starts_with("#extm3u") {
        return Ok(FileFormat::M3u);
    }
    // Basic M3U — content-free, so the extension is the only evidence.
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "m3u" | "m3u8" => Ok(FileFormat::M3u),
        "pls" => Ok(FileFormat::Pls),
        "xspf" => Ok(FileFormat::Xspf),
        _ => Err(PlaylistImportError::UnrecognizedFormat),
    }
}

/// A `FileN=` line anywhere in the head, for the headerless PLS case.
fn has_pls_key(lower_head: &str) -> bool {
    lower_head.split(['\n', '\r']).any(|line| {
        let line = line.trim_start();
        if !line.starts_with("file") {
            return false;
        }
        let rest = &line[4..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        !digits.is_empty() && rest[digits.len()..].trim_start().starts_with('=')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_is_sniffed_as_xspf_whatever_the_extension_says() {
        let body = br#"<?xml version="1.0"?><playlist><trackList/></playlist>"#;
        assert_eq!(detect_format(body, "mislabeled.m3u").unwrap(), FileFormat::Xspf);
    }

    #[test]
    fn a_pls_header_or_a_bare_file_key_is_enough() {
        assert_eq!(detect_format(b"[playlist]\nFile1=a\n", "x.txt").unwrap(), FileFormat::Pls);
        assert_eq!(detect_format(b"File1=a.flac\nTitle1=A\n", "x.txt").unwrap(), FileFormat::Pls);
    }

    #[test]
    fn extm3u_is_sniffed_and_a_bare_list_falls_back_to_the_extension() {
        assert_eq!(detect_format(b"#EXTM3U\na.flac\n", "x.txt").unwrap(), FileFormat::M3u);
        assert_eq!(detect_format(b"/music/a.flac\n", "list.m3u8").unwrap(), FileFormat::M3u);
    }

    #[test]
    fn an_unknown_extension_with_unknown_content_is_refused() {
        assert!(matches!(
            detect_format(b"just some prose\n", "notes.txt"),
            Err(PlaylistImportError::UnrecognizedFormat)
        ));
    }

    #[test]
    fn a_bom_does_not_hide_the_format() {
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(b"[playlist]\nFile1=a\n");
        assert_eq!(detect_format(&body, "x.pls").unwrap(), FileFormat::Pls);
    }

    #[test]
    fn oversize_is_refused_before_anything_else() {
        let big = vec![b'a'; super::super::MAX_IMPORT_BYTES + 1];
        assert!(matches!(
            detect_format(&big, "x.m3u"),
            Err(PlaylistImportError::FileTooLarge)
        ));
        assert!(matches!(
            parse(FileFormat::M3u, &big, "x.m3u"),
            Err(PlaylistImportError::FileTooLarge)
        ));
    }

    #[test]
    fn a_file_that_yields_no_tracks_is_an_error_not_an_empty_playlist() {
        assert!(matches!(
            parse(FileFormat::M3u, b"#EXTM3U\n", "x.m3u"),
            Err(PlaylistImportError::EmptyPlaylist)
        ));
    }
}
