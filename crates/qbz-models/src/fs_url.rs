//! `file://` URLs and "is this an absolute local path" — ONE definition.
//!
//! Sites used to build `format!("file://{p}")` by hand and to test
//! `starts_with('/')` for "absolute path". Both are Unix-only:
//! `file://C:\Users\…` parses `C:` as a URL **host**, and `C:\…` does not
//! start with `/`. Windows lost every local cover, the folder tree and the
//! runtime-tinted icons — silently, because a URL that resolves to nothing
//! renders as an empty rectangle, not as an error. Everything routes through
//! here now.
//!
//! # Two ambiguities no string algorithm can resolve
//!
//! A backslash is a legal filename character on Unix, and so is a colon. So
//! `C:\cover.jpg` and `/C:/music` are each simultaneously a valid Windows path
//! and a valid (if pathological) Unix one. This module reads them as Windows,
//! which costs a Unix user only if their path's FIRST component is a single
//! letter followed by a colon. The alternative — deciding by host — would make
//! the tests pass on the machine that runs them and fail on the one that
//! ships, which is the failure mode this module exists to end.

/// Absolute local path, by SHAPE (so the tests run on every host):
/// `/…` (POSIX), `X:\…` / `X:/…` (drive), `\\server\share…` (UNC).
///
/// Shape, not `Path::is_absolute`: that answers for the HOST it runs on, so a
/// Linux CI run would call `C:\x` relative and the Windows-only regression
/// would pass every test on the machine that runs them.
pub fn is_local_abs_path(s: &str) -> bool {
    s.starts_with('/') || is_drive_path(s) || s.starts_with("\\\\")
}

/// `X:\…` or `X:/…`. A separator is REQUIRED: `C:cover.jpg` is drive-RELATIVE
/// on Windows and an ordinary relative filename on Unix, so it is neither an
/// absolute path nor something to rewrite.
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Undo Windows extended-length ("verbatim") syntax.
///
/// `std::fs::canonicalize` RETURNS this form on Windows — documented, not
/// incidental — and `qbz-library/src/scan.rs` stores exactly that string for
/// every scanned file. Left alone, `\\?\C:\Music\cover.jpg` reads as a UNC
/// path whose server is `?`, and the URL comes out `file://%3F/C:/Music/…`:
/// the whole Local Library would have lost its covers on Windows.
fn undo_verbatim(path: &str) -> String {
    match path.strip_prefix("\\\\?\\") {
        // `\\?\UNC\server\share` is the verbatim spelling of `\\server\share`.
        Some(rest) => match rest.strip_prefix("UNC\\") {
            Some(unc) => format!("\\\\{unc}"),
            None => rest.to_string(),
        },
        None => path.to_string(),
    }
}

/// `file://` URL for a raw filesystem path. Idempotent for inputs that already
/// carry the scheme.
///
/// Qt's `QUrl` parses `#` as a fragment and `?` as a query, so a cover under
/// `…/Album #1/cover.jpg` resolves to nothing when concatenated raw.
/// Percent-encode exactly those two plus `%` itself (first, or the escapes we
/// add would be double-decoded). Spaces are left alone: `QUrl` accepts them.
///
/// Shapes:
/// * `/home/v/x` → `file:///home/v/x`
/// * `C:\Users\v\x` → `file:///C:/Users/v/x` (empty authority; what `QUrl` and
///   WinRT both accept)
/// * `\\nas\music\x` → `file://nas/music/x` (the server IS the authority, and
///   `QUrl::toLocalFile` restores `//nas/music/x` from exactly this)
/// * `rel/x` → `file://rel/x`, byte-identical to the hand-rolled code this
///   replaced. A relative path was never a valid `file://` URL; it is left
///   alone rather than "improved", because Linux behaviour does not move in a
///   Windows port.
pub fn file_url(path: &str) -> String {
    if path.starts_with("file://") {
        return path.to_string();
    }
    let path = undo_verbatim(path);

    // Backslash-to-slash ONLY for a Windows-shaped input. A backslash is a
    // legal filename character on Unix, so rewriting it unconditionally would
    // point `/home/v/AC\DC/cover.jpg` at a directory that does not exist.
    let win_shaped = is_drive_path(&path) || path.starts_with("\\\\");
    let normalised = if win_shaped {
        path.replace('\\', "/")
    } else {
        path
    };

    let mut out = String::with_capacity(normalised.len() + 8);
    out.push_str("file://");
    let body: &str = if let Some(unc) = normalised.strip_prefix("//") {
        // UNC: the server becomes the authority.
        unc
    } else if win_shaped {
        // A drive path needs the third slash for the empty authority.
        out.push('/');
        &normalised
    } else {
        // POSIX absolute already carries its own leading slash; a relative
        // path keeps the shape the old code produced.
        &normalised
    };
    for ch in body.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            '?' => out.push_str("%3F"),
            _ => out.push(ch),
        }
    }
    out
}

/// The inverse of [`file_url`]: a `file://` URL back to a filesystem path.
///
/// `trim_start_matches("file://")` is NOT the inverse — it leaves the three
/// percent-escapes in place, drops the `//` a UNC authority stands for, and on
/// Windows leaves a `/` before the drive letter. Each of those names a file
/// that does not exist, and the caller (a tint decode, a copy, an
/// `Image.source`) then fails silently.
pub fn local_path(url: &str) -> String {
    let Some(rest) = url.strip_prefix("file://") else {
        return url.to_string();
    };
    let raw = rest
        .replace("%23", "#")
        .replace("%3F", "?")
        .replace("%25", "%");
    let b = raw.as_bytes();
    // `file:///C:/x` → rest `/C:/x` → `C:/x`.
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return raw[1..].to_string();
    }
    // `file:///home/x` → rest `/home/x`: an empty authority, already a path.
    if raw.starts_with('/') || raw.is_empty() {
        return raw;
    }
    // A non-empty authority is a UNC server: `file://nas/music/x` names
    // `//nas/music/x`, which is what `QUrl::toLocalFile` returns too.
    format!("//{raw}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_paths_round_trip_and_escape() {
        assert_eq!(
            file_url("/home/v/Album #1/cover?.jpg"),
            "file:///home/v/Album %231/cover%3F.jpg"
        );
        assert_eq!(
            local_path("file:///home/v/Album%20%231/c%25.jpg"),
            "/home/v/Album%20#1/c%.jpg"
        );
        assert_eq!(file_url("file:///already"), "file:///already");
    }

    #[test]
    fn drive_paths_get_the_empty_authority_form() {
        assert_eq!(
            file_url("C:\\Users\\v\\cover.jpg"),
            "file:///C:/Users/v/cover.jpg"
        );
        assert_eq!(
            file_url("C:/Users/v/cover.jpg"),
            "file:///C:/Users/v/cover.jpg"
        );
        assert_eq!(
            local_path("file:///C:/Users/v/cover.jpg"),
            "C:/Users/v/cover.jpg"
        );
    }

    #[test]
    fn drive_paths_round_trip_through_both_directions() {
        // The regression that started this: a tinted icon directory.
        let p = "C:\\Users\\blitz\\AppData\\Local\\qbz\\icon-tints\\4285f4";
        let u = file_url(p);
        assert_eq!(
            u,
            "file:///C:/Users/blitz/AppData/Local/qbz/icon-tints/4285f4"
        );
        assert_eq!(
            local_path(&u),
            "C:/Users/blitz/AppData/Local/qbz/icon-tints/4285f4"
        );
        assert_eq!(file_url(&u), u, "file_url must be idempotent");
    }

    /// `std::fs::canonicalize` returns this shape on Windows and
    /// `qbz-library/src/scan.rs` stores it for every scanned file. Read as UNC
    /// it emits `file://%3F/C:/…` and the Local Library shows no covers at all.
    #[test]
    fn verbatim_extended_length_paths_are_undone() {
        assert_eq!(
            file_url("\\\\?\\C:\\Music\\Album\\cover.jpg"),
            "file:///C:/Music/Album/cover.jpg"
        );
        assert_eq!(
            file_url("\\\\?\\UNC\\nas\\music\\cover.jpg"),
            "file://nas/music/cover.jpg"
        );
    }

    #[test]
    fn unc_paths_put_the_server_in_the_authority_and_come_back() {
        let u = file_url("\\\\nas\\music\\cover.jpg");
        assert_eq!(u, "file://nas/music/cover.jpg");
        // Not `nas/music/cover.jpg`: that names nothing. Same as
        // QUrl::toLocalFile.
        assert_eq!(local_path(&u), "//nas/music/cover.jpg");
    }

    #[test]
    fn a_backslash_in_a_unix_filename_is_not_a_separator() {
        // Legal on Unix; rewriting it would name a path that does not exist.
        assert_eq!(
            file_url("/home/v/AC\\DC/cover.jpg"),
            "file:///home/v/AC\\DC/cover.jpg"
        );
    }

    /// A relative path is not a valid `file://` URL in any spelling. What
    /// matters is that Linux gets the SAME bytes the hand-rolled code gave it.
    #[test]
    fn relative_paths_keep_the_shape_the_old_code_produced() {
        assert_eq!(file_url("covers/x.jpg"), "file://covers/x.jpg");
        // Drive-RELATIVE is relative too, on both platforms.
        assert_eq!(file_url("C:cover.jpg"), "file://C:cover.jpg");
    }

    #[test]
    fn absolute_path_shape_is_host_independent() {
        assert!(is_local_abs_path("/x"));
        assert!(is_local_abs_path("C:\\x"));
        assert!(is_local_abs_path("c:/x"));
        assert!(is_local_abs_path("\\\\nas\\x"));
        assert!(!is_local_abs_path("x/y"));
        assert!(!is_local_abs_path("https://x"));
        assert!(!is_local_abs_path("Album|Artist"));
        assert!(!is_local_abs_path("C:x")); // drive-relative is not absolute
    }
}
