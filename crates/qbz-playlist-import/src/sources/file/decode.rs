//! Bytes -> text, and the two string chores every text format shares.

use crate::models::ImportTrack;

/// Strip a UTF-8 / UTF-16 BOM.
pub(crate) fn strip_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        &bytes[2..]
    } else {
        bytes
    }
}

/// Decode a legacy playlist body.
///
/// Strict UTF-8 first. On failure, WINDOWS-1252 — not because it is likely to
/// be right in the abstract, but because `.m3u` and `.pls` predate UTF-8 by a
/// decade and every Windows player that ever wrote one wrote 1252. Without the
/// fallback "Björk" and "Sigur Rós" arrive as replacement characters and stop
/// matching, which is a silent quality loss, not a crash.
///
/// UTF-16 is deliberately NOT attempted: the BOM is stripped above and a
/// UTF-16 body without one is indistinguishable from binary. Such a file falls
/// out at format detection as unrecognized, which is the honest answer.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    let body = strip_bom(bytes);
    match std::str::from_utf8(body) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (cow, _, _) = encoding_rs::WINDOWS_1252.decode(body);
            cow.into_owned()
        }
    }
}

/// Split lines on `\n`, `\r\n` AND a lone `\r` (classic Mac exports), trimming
/// each. `str::lines` does not handle the lone `\r`.
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// The `#EXTINF` / `TitleN` "Artist - Title" convention.
///
/// LOSSY, AND THE LOSS IS WORTH NAMING: split on the FIRST " - ", so
/// "Bob Dylan - Ballad of a Thin Man" is right and "Godspeed You! Black
/// Emperor - Dead Flag Blues - Part 2" keeps the tail in the title, which is
/// the better of the two mistakes available. A row with no " - " becomes
/// title-only with an empty artist — and the matcher tops out at 0.6 for a
/// title alone, below its 0.65 threshold, so such a row almost always lands in
/// `skipped_tracks`. That is the honest outcome of a format that never carried
/// the artist separately; it is reported, not hidden.
pub(crate) fn split_artist_title(raw: &str) -> (String, String) {
    let raw = raw.trim();
    match raw.split_once(" - ") {
        Some((artist, title)) => (artist.trim().to_string(), title.trim().to_string()),
        None => (String::new(), raw.to_string()),
    }
}

/// The filename without its directory or extension — the last-resort title,
/// and the playlist name when the format carries none.
pub(crate) fn file_stem(path: &str) -> String {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path);
    match name.rsplit_once('.') {
        // A leading dot is not an extension separator (".hidden").
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

/// A URL-decoded, directory-stripped stem for a `<location>` / file path.
pub(crate) fn location_stem(location: &str) -> String {
    let cleaned = location
        .trim()
        .trim_start_matches("file://")
        .split(['?', '#'])
        .next()
        .unwrap_or(location);
    file_stem(&percent_decode(cleaned))
}

/// Minimal percent-decoding. The crate has no `percent-encoding` dependency and
/// this is the only place that needs it: XSPF `<location>` is a URI, so
/// "Sigur%20R%C3%B3s.flac" has to come back as text before the stem is taken.
/// Invalid escapes are left verbatim rather than dropped.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A track with a derived title, or `None` when even the fallbacks are empty.
/// The ONE skip rule shared by the three file parsers.
pub(crate) fn track_or_skip(
    title: String,
    artist: String,
    album: Option<String>,
    duration_ms: Option<u64>,
    isrc: Option<String>,
) -> Option<ImportTrack> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return None;
    }
    Some(ImportTrack {
        title,
        artist: artist.trim().to_string(),
        album: album.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
        duration_ms,
        isrc,
        provider_id: None,
        provider_url: None,
    })
}

/// Uppercase + strip every non-alphanumeric, then accept only the 12-character
/// ISRC shape.
///
/// NORMALIZATION IS REQUIRED, not cosmetic: `match_qobuz.rs` compares with
/// `eq_ignore_ascii_case` and NO hyphen stripping, while Qobuz stores the bare
/// 12-character form. A tagged "US-KO1-16-00123" would therefore never fire the
/// score-1.0 fast path — the single highest-value match this whole pipeline has.
pub(crate) fn normalize_isrc(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if is_isrc_shaped(&cleaned) {
        Some(cleaned)
    } else {
        None
    }
}

/// `CCXXXYYNNNNN` — 2 letters, 3 alphanumerics, 7 digits.
pub(crate) fn is_isrc_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 12
        && b[0..2].iter().all(|c| c.is_ascii_alphabetic())
        && b[2..5].iter().all(|c| c.is_ascii_alphanumeric())
        && b[5..12].iter().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bom_is_stripped_and_utf8_survives() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("Björk".as_bytes());
        assert_eq!(decode_text(&bytes), "Björk");
    }

    #[test]
    fn windows_1252_accents_are_recovered() {
        // "Sigur Rós" in Windows-1252: ó is 0xF3, which is invalid UTF-8 alone.
        let bytes = b"Sigur R\xF3s".to_vec();
        assert!(std::str::from_utf8(&bytes).is_err());
        assert_eq!(decode_text(&bytes), "Sigur Rós");
    }

    #[test]
    fn lines_split_on_lone_cr() {
        assert_eq!(split_lines("a\rb\r\nc\nd"), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn artist_title_splits_on_the_first_dash_only() {
        assert_eq!(
            split_artist_title("Bob Dylan - Ballad of a Thin Man"),
            ("Bob Dylan".to_string(), "Ballad of a Thin Man".to_string())
        );
        // The tail stays with the title.
        assert_eq!(
            split_artist_title("GY!BE - Dead Flag Blues - Part 2"),
            ("GY!BE".to_string(), "Dead Flag Blues - Part 2".to_string())
        );
        // No separator: title only, empty artist.
        assert_eq!(
            split_artist_title("Untitled"),
            (String::new(), "Untitled".to_string())
        );
        // A hyphen without spaces is NOT the separator.
        assert_eq!(
            split_artist_title("Jay-Z"),
            (String::new(), "Jay-Z".to_string())
        );
    }

    #[test]
    fn stems_drop_directory_and_extension() {
        assert_eq!(file_stem("/music/Road Trip.m3u"), "Road Trip");
        assert_eq!(file_stem("C:\\lists\\Party.pls"), "Party");
        assert_eq!(file_stem("plain"), "plain");
        assert_eq!(file_stem(".hidden"), ".hidden");
    }

    #[test]
    fn location_stem_percent_decodes() {
        assert_eq!(
            location_stem("file:///music/Sigur%20R%C3%B3s%20-%20Hoppipolla.flac"),
            "Sigur Rós - Hoppipolla"
        );
        // A bad escape survives verbatim rather than eating the string.
        assert_eq!(location_stem("/m/100%.flac"), "100%");
    }

    #[test]
    fn isrc_normalizes_to_the_bare_form_qobuz_stores() {
        assert_eq!(
            normalize_isrc("US-KO1-16-00123").as_deref(),
            Some("USKO11600123")
        );
        assert_eq!(normalize_isrc("usko11600123").as_deref(), Some("USKO11600123"));
        // A MusicBrainz recording id is NOT an ISRC and must never pass — it
        // would misfire the score-1.0 short circuit.
        assert_eq!(normalize_isrc("b1a9c0e9-1b47-4f3d-9a1e-000000000000"), None);
        assert_eq!(normalize_isrc("too-short"), None);
    }
}
