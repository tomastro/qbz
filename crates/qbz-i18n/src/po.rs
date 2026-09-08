//! Minimal gettext `.po` parser producing a [`Catalog`].
//!
//! Keyed by `msgid` (the English source string); we do NOT use `msgctxt`.
//! Handles: the header entry (`msgid ""`), `msgid` / `msgid_plural` /
//! `msgstr` / `msgstr[N]`, multi-line string continuations (adjacent quoted
//! lines concatenate), `#` comment lines, and `\n` / `\t` / `\"` / `\\` escapes.
//! An empty `msgstr` means "no translation" → lookups return `None` so callers
//! fall back to the English source.

use std::collections::HashMap;

use crate::plural::PluralRule;

/// A parsed translation catalog for a single language.
#[derive(Debug, Clone)]
pub struct Catalog {
    lang: String,
    plural_rule: PluralRule,
    /// Singular: msgid -> msgstr (non-empty only).
    singular: HashMap<String, String>,
    /// Plural: msgid -> Vec of msgstr[N] (index = plural form).
    plural: HashMap<String, Vec<String>>,
}

impl Catalog {
    /// Parse `.po` text for the given language code.
    pub fn parse(lang: &str, po_text: &str) -> Catalog {
        let mut singular: HashMap<String, String> = HashMap::new();
        let mut plural: HashMap<String, Vec<String>> = HashMap::new();
        let mut plural_rule = PluralRule::default();

        let mut cur = Entry::default();

        // Track which field the current continuation lines belong to.
        for raw_line in po_text.lines() {
            let line = raw_line.trim();

            if line.is_empty() {
                flush(&mut cur, &mut singular, &mut plural, &mut plural_rule);
                continue;
            }
            if line.starts_with('#') {
                continue;
            }

            if let Some(rest) = line.strip_prefix("msgctxt ") {
                // We ignore msgctxt for keying but still consume its string so a
                // trailing continuation doesn't bleed into the wrong field.
                let _ = parse_quoted(rest);
                cur.last = Field::None;
            } else if let Some(rest) = line.strip_prefix("msgid_plural ") {
                cur.msgid_plural = Some(parse_quoted(rest).unwrap_or_default());
                cur.last = Field::MsgidPlural;
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                // A new msgid begins a new entry; flush any pending one.
                flush(&mut cur, &mut singular, &mut plural, &mut plural_rule);
                cur.msgid = Some(parse_quoted(rest).unwrap_or_default());
                cur.last = Field::Msgid;
            } else if let Some(rest) = line.strip_prefix("msgstr[") {
                // msgstr[N] "..."
                if let Some(close) = rest.find(']') {
                    let n: usize = rest[..close].trim().parse().unwrap_or(0);
                    let after = rest[close + 1..].trim_start();
                    let val = parse_quoted(after).unwrap_or_default();
                    if cur.msgstr_plural.len() <= n {
                        cur.msgstr_plural.resize(n + 1, String::new());
                    }
                    cur.msgstr_plural[n] = val;
                    cur.last = Field::MsgstrPlural(n);
                }
            } else if let Some(rest) = line.strip_prefix("msgstr ") {
                cur.msgstr = Some(parse_quoted(rest).unwrap_or_default());
                cur.last = Field::Msgstr;
            } else if line.starts_with('"') {
                // Continuation line: append to the most recently seen field.
                let piece = parse_quoted(line).unwrap_or_default();
                match cur.last {
                    Field::Msgid => {
                        cur.msgid.get_or_insert_with(String::new).push_str(&piece)
                    }
                    Field::MsgidPlural => cur
                        .msgid_plural
                        .get_or_insert_with(String::new)
                        .push_str(&piece),
                    Field::Msgstr => {
                        cur.msgstr.get_or_insert_with(String::new).push_str(&piece)
                    }
                    Field::MsgstrPlural(n) => {
                        if n < cur.msgstr_plural.len() {
                            cur.msgstr_plural[n].push_str(&piece);
                        }
                    }
                    Field::None => {}
                }
            }
        }
        // Flush trailing entry (file may not end with a blank line).
        flush(&mut cur, &mut singular, &mut plural, &mut plural_rule);

        Catalog {
            lang: lang.to_string(),
            plural_rule,
            singular,
            plural,
        }
    }

    /// Language code this catalog was parsed for.
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// The catalog's plural rule (from the header `Plural-Forms`).
    pub fn plural_rule(&self) -> PluralRule {
        self.plural_rule
    }

    /// Number of plural forms for this catalog.
    pub fn nplurals(&self) -> usize {
        self.plural_rule.nplurals()
    }

    /// Look up the translated singular for `msgid`.
    /// Returns `None` when there is no (non-empty) translation.
    pub fn get(&self, msgid: &str) -> Option<&str> {
        self.singular.get(msgid).map(|s| s.as_str())
    }

    /// Look up the translated plural form `form_index` for `msgid`.
    /// Returns `None` when missing or empty.
    pub fn get_plural(&self, msgid: &str, form_index: usize) -> Option<&str> {
        let forms = self.plural.get(msgid)?;
        let val = forms.get(form_index)?;
        if val.is_empty() {
            None
        } else {
            Some(val.as_str())
        }
    }
}

/// Which field the parser last touched (for continuation lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    Msgid,
    MsgidPlural,
    Msgstr,
    MsgstrPlural(usize),
}

impl Default for Field {
    fn default() -> Self {
        Field::None
    }
}

#[derive(Debug, Default)]
struct Entry {
    msgid: Option<String>,
    msgid_plural: Option<String>,
    msgstr: Option<String>,
    msgstr_plural: Vec<String>,
    last: Field,
}

fn flush(
    cur: &mut Entry,
    singular: &mut HashMap<String, String>,
    plural: &mut HashMap<String, Vec<String>>,
    plural_rule: &mut PluralRule,
) {
    let entry = std::mem::take(cur);
    let msgid = match entry.msgid {
        Some(m) => m,
        None => return,
    };

    // Header entry: empty msgid carries metadata in its msgstr.
    if msgid.is_empty() {
        if let Some(header) = entry.msgstr {
            for line in header.split('\n') {
                if let Some(value) = line.strip_prefix("Plural-Forms:") {
                    *plural_rule = PluralRule::parse(value.trim());
                }
            }
        }
        return;
    }

    if entry.msgid_plural.is_some() || !entry.msgstr_plural.is_empty() {
        plural.insert(msgid, entry.msgstr_plural);
    } else if let Some(s) = entry.msgstr {
        if !s.is_empty() {
            singular.insert(msgid, s);
        }
    }
}

/// Extract and unescape the contents of a leading `"..."` segment.
fn parse_quoted(s: &str) -> Option<String> {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break, // closing quote
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => break,
            },
            other => out.push(other),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
msgid ""
msgstr ""
"Language: es\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

# a simple singular
msgid "Play"
msgstr "Reproducir"

# an empty translation -> no translation
msgid "Pause"
msgstr ""

# a plural entry
msgid "{} track"
msgid_plural "{} tracks"
msgstr[0] "{} pista"
msgstr[1] "{} pistas"
"#;

    #[test]
    fn parses_singular() {
        let cat = Catalog::parse("es", SAMPLE);
        assert_eq!(cat.get("Play"), Some("Reproducir"));
    }

    #[test]
    fn empty_msgstr_is_none() {
        let cat = Catalog::parse("es", SAMPLE);
        assert_eq!(cat.get("Pause"), None);
    }

    #[test]
    fn missing_msgid_is_none() {
        let cat = Catalog::parse("es", SAMPLE);
        assert_eq!(cat.get("Stop"), None);
    }

    #[test]
    fn parses_plural_forms() {
        let cat = Catalog::parse("es", SAMPLE);
        assert_eq!(cat.get_plural("{} track", 0), Some("{} pista"));
        assert_eq!(cat.get_plural("{} track", 1), Some("{} pistas"));
        assert_eq!(cat.get_plural("{} track", 2), None);
    }

    #[test]
    fn reads_nplurals_from_header() {
        let cat = Catalog::parse("es", SAMPLE);
        assert_eq!(cat.nplurals(), 2);
        // (n != 1): 1 -> form 0, else form 1.
        assert_eq!(cat.plural_rule().index(1), 0);
        assert_eq!(cat.plural_rule().index(3), 1);
    }

    #[test]
    fn handles_multiline_continuation_and_escapes() {
        let po = r#"
msgid "greeting"
msgstr "Hello "
"world\nLine\ttab \"q\" back\\slash"
"#;
        let cat = Catalog::parse("en", po);
        assert_eq!(
            cat.get("greeting"),
            Some("Hello world\nLine\ttab \"q\" back\\slash")
        );
    }
}
