//! Convert HTML-ish strings from Qobuz (biographies, album reviews)
//! into Slint-friendly plain text. Slint's `Text` is single-style, so
//! we cannot render inline strong/em formatting — those tags are
//! stripped but their content stays inline. Paragraph and line-break
//! structure IS preserved: `<br>` collapses to `\n`, `</p>` to a
//! blank line so Text renders the paragraphs separated visually.

/// Render an HTML-ish blurb into plain text with paragraph breaks.
pub fn strip_html(input: &str) -> String {
    let normalized = normalize_breaks(input);
    let stripped = strip_remaining_tags(&normalized);
    let decoded = decode_entities(&stripped);
    collapse_blank_lines(&decoded)
}

/// Walk by char (not byte) so multi-byte UTF-8 sequences (ó, é, "—",
/// curly quotes) survive untouched. Skip recognized `<br>` and `</p>`
/// runs by replacing them with newlines; pass everything else through
/// so the second pass can strip the remaining tags.
fn normalize_breaks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix('<') {
            if let Some((replacement, consumed)) = match_break_or_paragraph(stripped) {
                out.push_str(replacement);
                rest = &stripped[consumed..];
                continue;
            }
        }
        // Advance one char (not one byte) — pushes the full UTF-8
        // sequence intact.
        let mut chars = rest.chars();
        if let Some(ch) = chars.next() {
            out.push(ch);
            rest = chars.as_str();
        } else {
            break;
        }
    }
    out
}

/// Try to match `<br>` (any case, with optional spaces and self-
/// closing slash) or `</p>` (any case). `s` starts AFTER the opening
/// `<`. Returns the replacement string + bytes consumed (after the
/// closing `>`).
fn match_break_or_paragraph(s: &str) -> Option<(&'static str, usize)> {
    let bytes = s.as_bytes();
    // </p>
    if bytes.len() >= 3
        && bytes[0] == b'/'
        && (bytes[1] == b'p' || bytes[1] == b'P')
        && bytes[2] == b'>'
    {
        return Some(("\n\n", 3));
    }
    // <br>, <br/>, <br />, etc.
    if bytes.len() >= 3 && (bytes[0] == b'b' || bytes[0] == b'B')
        && (bytes[1] == b'r' || bytes[1] == b'R')
    {
        let mut j = 2usize;
        while j < bytes.len() && bytes[j] != b'>' {
            // Only allow whitespace and a single '/' between `br` and `>`.
            if !bytes[j].is_ascii_whitespace() && bytes[j] != b'/' {
                return None;
            }
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'>' {
            return Some(("\n", j + 1));
        }
    }
    None
}

/// Drop all remaining tags but keep their text content. Char-safe.
fn strip_remaining_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Decode HTML entities into plain text. Public because some API prose
/// fields carry entities without any markup (e.g. the artist biography
/// source credit) — those callers want entity decoding WITHOUT the
/// tag-strip / paragraph passes of [`strip_html`].
///
/// Handles:
/// - the named entities Qobuz/TiVo prose actually emits (see `NAMED`),
///   incl. the full Latin-1 accented set;
/// - numeric character references, decimal (`&#233;`) and hex (`&#xE9;`),
///   with the Windows-1252 quirk range (`&#146;` → ’) mapped like
///   browsers do;
/// - MALFORMED no-semicolon forms for a tiny allowlist (`&copy` `&reg`
///   `&amp` `&nbsp`) when followed by a word boundary — TiVo biography
///   credits really arrive as `&copy  John Book /TiVo` (no semicolon).
///   Tradeoff: prose that literally means the string "&copy " would be
///   rewritten; accepted, since that never appears in catalog prose,
///   while the malformed credit line appears on virtually every bio.
///   Names outside the allowlist ("AC&DC", "&copyright2020") are never
///   touched without their semicolon.
pub fn decode_html_entities(input: &str) -> String {
    decode_entities(input)
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while !rest.is_empty() {
        if rest.as_bytes()[0] == b'&' {
            if let Some((decoded, consumed)) = match_entity(&rest[1..]) {
                out.push(decoded);
                rest = &rest[1 + consumed..];
                continue;
            }
        }
        let mut chars = rest.chars();
        if let Some(ch) = chars.next() {
            out.push(ch);
            rest = chars.as_str();
        } else {
            break;
        }
    }
    out
}

/// Named entities → their (single) character. Semicolon-terminated form
/// only; the bare tolerance is limited to `BARE_NAMES`.
const NAMED: &[(&str, char)] = &[
    // Core escapes
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", ' '),
    // Symbols
    ("copy", '\u{00A9}'),
    ("reg", '\u{00AE}'),
    ("trade", '\u{2122}'),
    ("deg", '\u{00B0}'),
    ("middot", '\u{00B7}'),
    ("bull", '\u{2022}'),
    ("sect", '\u{00A7}'),
    ("para", '\u{00B6}'),
    ("plusmn", '\u{00B1}'),
    ("times", '\u{00D7}'),
    ("divide", '\u{00F7}'),
    ("euro", '\u{20AC}'),
    ("pound", '\u{00A3}'),
    ("cent", '\u{00A2}'),
    ("yen", '\u{00A5}'),
    // Dashes / ellipsis / quotes
    ("mdash", '\u{2014}'),
    ("ndash", '\u{2013}'),
    ("hellip", '\u{2026}'),
    ("ldquo", '\u{201C}'),
    ("rdquo", '\u{201D}'),
    ("lsquo", '\u{2018}'),
    ("rsquo", '\u{2019}'),
    ("laquo", '\u{00AB}'),
    ("raquo", '\u{00BB}'),
    // Latin-1 accented, lowercase
    ("agrave", '\u{00E0}'),
    ("aacute", '\u{00E1}'),
    ("acirc", '\u{00E2}'),
    ("atilde", '\u{00E3}'),
    ("auml", '\u{00E4}'),
    ("aring", '\u{00E5}'),
    ("aelig", '\u{00E6}'),
    ("ccedil", '\u{00E7}'),
    ("egrave", '\u{00E8}'),
    ("eacute", '\u{00E9}'),
    ("ecirc", '\u{00EA}'),
    ("euml", '\u{00EB}'),
    ("igrave", '\u{00EC}'),
    ("iacute", '\u{00ED}'),
    ("icirc", '\u{00EE}'),
    ("iuml", '\u{00EF}'),
    ("ntilde", '\u{00F1}'),
    ("ograve", '\u{00F2}'),
    ("oacute", '\u{00F3}'),
    ("ocirc", '\u{00F4}'),
    ("otilde", '\u{00F5}'),
    ("ouml", '\u{00F6}'),
    ("oslash", '\u{00F8}'),
    ("ugrave", '\u{00F9}'),
    ("uacute", '\u{00FA}'),
    ("ucirc", '\u{00FB}'),
    ("uuml", '\u{00FC}'),
    ("yacute", '\u{00FD}'),
    ("yuml", '\u{00FF}'),
    ("szlig", '\u{00DF}'),
    ("oelig", '\u{0153}'),
    // Latin-1 accented, uppercase
    ("Agrave", '\u{00C0}'),
    ("Aacute", '\u{00C1}'),
    ("Acirc", '\u{00C2}'),
    ("Atilde", '\u{00C3}'),
    ("Auml", '\u{00C4}'),
    ("Aring", '\u{00C5}'),
    ("AElig", '\u{00C6}'),
    ("Ccedil", '\u{00C7}'),
    ("Egrave", '\u{00C8}'),
    ("Eacute", '\u{00C9}'),
    ("Ecirc", '\u{00CA}'),
    ("Euml", '\u{00CB}'),
    ("Igrave", '\u{00CC}'),
    ("Iacute", '\u{00CD}'),
    ("Icirc", '\u{00CE}'),
    ("Iuml", '\u{00CF}'),
    ("Ntilde", '\u{00D1}'),
    ("Ograve", '\u{00D2}'),
    ("Oacute", '\u{00D3}'),
    ("Ocirc", '\u{00D4}'),
    ("Otilde", '\u{00D5}'),
    ("Ouml", '\u{00D6}'),
    ("Oslash", '\u{00D8}'),
    ("Ugrave", '\u{00D9}'),
    ("Uacute", '\u{00DA}'),
    ("Ucirc", '\u{00DB}'),
    ("Uuml", '\u{00DC}'),
    ("Yacute", '\u{00DD}'),
    ("OElig", '\u{0152}'),
];

/// Entities the Qobuz API emits WITHOUT the trailing semicolon (the TiVo
/// `&copy  Name /TiVo` credit line). Kept deliberately tiny — every name
/// added here widens the false-positive surface on plain-text ampersands.
const BARE_NAMES: &[&str] = &["copy", "reg", "amp", "nbsp"];

/// Match one entity at `s`, which starts AFTER the `&`. Returns the
/// decoded char + bytes consumed (after the `&`).
fn match_entity(s: &str) -> Option<(char, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'#') {
        return match_numeric(&s[1..]).map(|(ch, used)| (ch, used + 1));
    }
    // Read the maximal alphanumeric name run (entity names are ASCII).
    let name_len = bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    if name_len == 0 || name_len > 8 {
        return None;
    }
    let name = &s[..name_len];
    let decoded = NAMED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ch)| *ch)?;
    if bytes.get(name_len) == Some(&b';') {
        return Some((decoded, name_len + 1));
    }
    // Malformed no-semicolon tolerance: allowlisted names only, and only
    // when the next char is a word boundary (end of text, whitespace or
    // punctuation) — so "&copyright" or "AC&DCfan" never decode.
    let at_boundary = match s[name_len..].chars().next() {
        None => true,
        Some(next) => !next.is_alphanumeric(),
    };
    if at_boundary && BARE_NAMES.contains(&name) {
        return Some((decoded, name_len));
    }
    None
}

/// Numeric character reference. `s` starts AFTER `&#`. The semicolon is
/// REQUIRED here — a bare `&#169 ` is left literal (unlike the named
/// allowlist, malformed numeric forms haven't been observed in the wild
/// and digits-then-space appears in ordinary prose too easily).
fn match_numeric(s: &str) -> Option<(char, usize)> {
    let bytes = s.as_bytes();
    let (radix, digits_start) = match bytes.first() {
        Some(b'x') | Some(b'X') => (16u32, 1usize),
        _ => (10u32, 0usize),
    };
    let mut value: u32 = 0;
    let mut i = digits_start;
    while i < bytes.len() {
        let Some(d) = (bytes[i] as char).to_digit(radix) else { break };
        value = value.checked_mul(radix)?.checked_add(d)?;
        if value > 0x10FFFF {
            return None;
        }
        i += 1;
    }
    if i == digits_start || bytes.get(i) != Some(&b';') {
        return None;
    }
    // Browsers map the C1 control range through Windows-1252 (CMS-sourced
    // text really contains `&#146;` for ’); mirror the common cases.
    let value = match value {
        0x82 => 0x201A, // ‚
        0x84 => 0x201E, // „
        0x85 => 0x2026, // …
        0x91 => 0x2018, // '
        0x92 => 0x2019, // '
        0x93 => 0x201C, // "
        0x94 => 0x201D, // "
        0x95 => 0x2022, // •
        0x96 => 0x2013, // –
        0x97 => 0x2014, // —
        0x99 => 0x2122, // ™
        v => v,
    };
    // Reject other control chars — decoding them into UI text helps nobody.
    let ch = char::from_u32(value)?;
    if ch.is_control() && ch != '\n' && ch != '\t' {
        return None;
    }
    Some((ch, i + 1))
}

fn collapse_blank_lines(input: &str) -> String {
    let trimmed = input.trim();
    let mut out = String::with_capacity(trimmed.len());
    let mut consecutive_newlines = 0;
    for ch in trimmed.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                out.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_inline_formatting() {
        let html = "<p>One <strong>bold</strong> and <em>italic</em>.</p>";
        let plain = strip_html(html);
        assert_eq!(plain, "One bold and italic.");
    }

    #[test]
    fn converts_br_to_newline() {
        let html = "Line 1<br>Line 2<br />Line 3";
        assert_eq!(strip_html(html), "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn converts_paragraphs() {
        let html = "<p>First.</p><p>Second.</p>";
        assert_eq!(strip_html(html), "First.\n\nSecond.");
    }

    #[test]
    fn decodes_common_entities() {
        let html = "Rock &amp; Roll &mdash; &ldquo;the rest&rdquo;.";
        assert_eq!(strip_html(html), "Rock & Roll \u{2014} \u{201C}the rest\u{201D}.");
    }

    #[test]
    fn preserves_multibyte_characters() {
        // Mexican Spanish with accented chars, ñ, ó — the previous
        // byte-walking implementation would have shredded these into
        // their UTF-8 bytes (à+³ instead of ó).
        let html = "<p>La cantautora se estableció en Madrid, España.</p>";
        let plain = strip_html(html);
        assert_eq!(plain, "La cantautora se estableció en Madrid, España.");
    }

    #[test]
    fn collapses_excess_newlines() {
        let html = "<p>A</p><p>B</p><p>C</p>";
        let out = strip_html(html);
        assert_eq!(out, "A\n\nB\n\nC");
    }

    // ---------- decode_html_entities ----------

    #[test]
    fn decodes_numeric_references() {
        assert_eq!(decode_html_entities("caf&#233;"), "café");
        assert_eq!(decode_html_entities("caf&#xE9;"), "café");
        assert_eq!(decode_html_entities("caf&#XE9;"), "café");
        assert_eq!(decode_html_entities("&#169; 2026"), "\u{00A9} 2026");
    }

    #[test]
    fn decodes_accented_named_entities() {
        assert_eq!(
            decode_html_entities("Beyonc&eacute; &amp; M&ouml;tley Cr&uuml;e"),
            "Beyoncé & Mötley Crüe"
        );
        assert_eq!(decode_html_entities("&Eacute;douard"), "Édouard");
    }

    #[test]
    fn decodes_malformed_bare_copy() {
        // The real TiVo credit line: no semicolon, double space
        // (qbz-nix-docs/qobuz-api/page-artist-response.json).
        assert_eq!(
            decode_html_entities("&copy  Mariano Prunes /TiVo"),
            "\u{00A9}  Mariano Prunes /TiVo"
        );
        assert_eq!(
            decode_html_entities("&copy John Book /TiVo"),
            "\u{00A9} John Book /TiVo"
        );
        // Bare form at end of text.
        assert_eq!(decode_html_entities("text &copy"), "text \u{00A9}");
        // Bare &amp / &nbsp / &reg at a boundary.
        assert_eq!(decode_html_entities("Tom &amp Jerry"), "Tom & Jerry");
        assert_eq!(decode_html_entities("QBZ&reg!"), "QBZ\u{00AE}!");
    }

    #[test]
    fn no_false_positives_on_plain_ampersands() {
        // Names not in the table never decode without a semicolon.
        assert_eq!(decode_html_entities("AC&DC"), "AC&DC");
        assert_eq!(decode_html_entities("R&B and Rhythm&Blues"), "R&B and Rhythm&Blues");
        // Allowlisted prefix but no word boundary → untouched.
        assert_eq!(decode_html_entities("&copyright2020"), "&copyright2020");
        assert_eq!(decode_html_entities("&amplifier"), "&amplifier");
        // Unknown entity with semicolon stays literal.
        assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
        // Numeric without semicolon stays literal (documented tradeoff).
        assert_eq!(decode_html_entities("&#169 2026"), "&#169 2026");
        // Trailing lone ampersand.
        assert_eq!(decode_html_entities("fish & chips &"), "fish & chips &");
    }

    #[test]
    fn clean_text_is_untouched_and_decode_is_stable() {
        let clean = "Ya quedó: café, señor — “quotes” & nothing else… ©";
        assert_eq!(decode_html_entities(clean), clean);
        // Idempotence on typical decoded prose (no re-encoding artifacts).
        let once = decode_html_entities("Beyonc&eacute; &copy John &#8212; ok");
        assert_eq!(decode_html_entities(&once), once);
    }

    #[test]
    fn decodes_windows_1252_quirk_range() {
        assert_eq!(decode_html_entities("It&#146;s here"), "It\u{2019}s here");
        assert_eq!(decode_html_entities("&#147;quoted&#148;"), "\u{201C}quoted\u{201D}");
    }

    #[test]
    fn full_pipeline_on_real_bio_tail() {
        // Verbatim shape of the Metallica bio tail in the API sample —
        // the literal \n plus <br /> yield a blank line before the credit.
        let html = "apareció en 2023.\n<br />&copy  Mariano Prunes /TiVo";
        assert_eq!(
            strip_html(html),
            "apareció en 2023.\n\n\u{00A9}  Mariano Prunes /TiVo"
        );
    }
}
