//! Qobuz lyrics wire DTOs.
//!
//! Two-step contract (inferred spec `qobuz-api-inferred-openapi-v9.9.0.0-beta.yaml:759-806`,
//! reconciled against LIVE captures 2026-06-10 — samples in
//! `qbz-nix-docs/qobuz-api/lyrics-*.json`):
//!   1. `GET /track/lyricsUrl` (signed RPC) -> [`QobuzLyricsUrls`] envelope with the CDN URL.
//!   2. `GET <lyrics_url>` (pre-signed CloudFront URL) -> [`QobuzLyricsDocument`].
//!
//! Divergences found live vs the yaml (each noted on the affected field):
//!   - Step-1 `track_id` is a JSON NUMBER (yaml said string); `album_id` IS a string.
//!   - The CDN payload is NOT the bare content union the yaml describes
//!     (`LyricsContentDto`): it is a wrapper `{album_id, track_id,
//!     translation_langs, publishers, writers, original}` with the content
//!     union under `original`.
//!   - The synced discriminator is `"wsync"` (word-synced), not `"lsync"`:
//!     each line carries per-WORD timestamps in addition to line start/end.
//!   - Plain lines are objects `{"line": "..."}`, not bare strings.
//!   - Miss = HTTP 404 with `{"status":"error","code":404,"message":"Lyrics
//!     are not available for this track."}` (sample: lyrics-miss-response.json).
//!
//! All fields stay serde-tolerant (`Option` / `#[serde(default)]`) — the
//! endpoint is a beta-era delta and may still move.

use serde::{Deserialize, Deserializer, Serialize};

/// Accept either a JSON string or a JSON number and normalize to `String`.
///
/// The yaml declares `track_id` as string ("string form, even though the query
/// param is numeric", yaml:205) but the live capture (lyrics-url-response.json)
/// shows `track_id` as a JSON NUMBER while `album_id` is a string (album slug
/// or barcode, e.g. `"gvcirtodd95kc"` / `"5052205066164"`). Tolerate both.
fn de_opt_string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(serde_json::Number),
    }

    Ok(Option::<StringOrNumber>::deserialize(deserializer)?.map(|v| match v {
        StringOrNumber::String(s) => s,
        StringOrNumber::Number(n) => n.to_string(),
    }))
}

/// Step-1 envelope returned by `GET /track/lyricsUrl`.
///
/// Yaml `LyricsUrlsDto` (`:194-219`). Live shape (lyrics-url-response.json):
/// `{"track_id": 266725027, "album_id": "gvcirtodd95kc", "lyrics_url": "https://…cloudfront.net/…"}`
/// — `track_id` is a number (yaml said string), `translation_requested` is
/// simply ABSENT when no translation was asked, and the URL is a pre-signed
/// CloudFront URL (self-authorizing: `Expires`/`Signature`/`Key-Pair-Id`).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsUrls {
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub track_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub album_id: Option<String>,
    /// URL to the original-language lyrics document (Qobuz CDN host).
    #[serde(default)]
    pub lyrics_url: Option<String>,
    /// Present only when the `language` query param was sent AND a matching
    /// translation exists (yaml:208-213). Absent when no translation was
    /// requested (confirmed absent in the live capture) or none is available
    /// for the requested language.
    #[serde(default)]
    pub translation_requested: Option<QobuzLyricsTranslation>,
}

/// Translation pointer inside [`QobuzLyricsUrls`] (yaml `LyricsTranslationDto`, `:221-231`).
/// Kept yaml-shaped + tolerant.
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsTranslation {
    #[serde(default)]
    pub lyrics_url: Option<String>,
    /// ISO 639-1 code of the returned translation.
    #[serde(default)]
    pub lang: Option<String>,
}

/// Step-2 CDN lyrics document — the REAL wire shape (live divergence: the
/// yaml's `LyricsContentDto` union is nested under `original`, wrapped with
/// catalog metadata; samples lyrics-doc-wsync.json / lyrics-doc-plain.json).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsDocument {
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub track_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string_or_number")]
    pub album_id: Option<String>,
    /// ISO 639-1 codes a translation exists for (e.g. `["pt","de","fr","es","it"]`).
    #[serde(default)]
    pub translation_langs: Vec<String>,
    /// Publisher copyright entries with applicability zones. Parsed but never
    /// enforced (owner decision 2026-07-22: no zone/copyright gating).
    #[serde(default)]
    pub publishers: Vec<QobuzLyricsPublisher>,
    /// Songwriter credits as a single display string.
    #[serde(default)]
    pub writers: Option<String>,
    /// The actual lyrics content (the yaml's "LyricsContentDto" union).
    #[serde(default)]
    pub original: Option<QobuzLyricsContent>,
    /// Translated lyrics content (NEW in API v10) — same union shape as
    /// `original`. LIVE WIRE (verified 2026-07-22): when `language` is
    /// requested and available, the step-1 `lyrics_url` itself points at the
    /// TRANSLATED document (`<id>-<lang>.json`), which carries ONLY this
    /// `translation` block and no `original` — the client must fetch the
    /// original document separately and merge. `translation_requested` was
    /// not observed in practice (kept as a fallback source only).
    #[serde(default)]
    pub translation: Option<QobuzLyricsContent>,
}

/// Merge a translation-only document into its original document (v10 live
/// wire: a `language` request returns a document carrying ONLY `translation`).
/// Moves the `translation` block over and unions `translation_langs`.
/// `original` keeps its own content/writers/publishers.
pub fn merge_translation_into(
    original: &mut QobuzLyricsDocument,
    mut translated: QobuzLyricsDocument,
) {
    original.translation = translated.translation.take();
    for lang in translated.translation_langs.drain(..) {
        if !original.translation_langs.contains(&lang) {
            original.translation_langs.push(lang);
        }
    }
}

/// One publisher credit inside [`QobuzLyricsDocument`] (live-only; not in the yaml).
#[derive(Debug, Clone, Deserialize)]
pub struct QobuzLyricsPublisher {
    #[serde(default)]
    pub copyright: Option<String>,
    /// Zone codes the copyright applies to (e.g. `["WW"]`, `["WW","FR"]`).
    #[serde(default)]
    pub zones: Vec<String>,
}

/// Lyrics content union, discriminated on `type`.
///
/// Live discriminator is `"wsync"` (word-synced — richer than the yaml's
/// `"lsync"` guess: per-word timestamps inside each line). `"lsync"` is kept
/// as an alias in case line-only documents exist in the catalog (same shape
/// minus `words`, which defaults to empty). `Serialize` exists so the lyrics
/// cache can persist a document's embedded translation verbatim.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum QobuzLyricsContent {
    /// Time-synced lyrics; line + per-word timestamps, ms from track start.
    #[serde(rename = "wsync", alias = "lsync")]
    Synced {
        /// ISO 639-1 of the document language (present in live captures).
        #[serde(default)]
        lang: Option<String>,
        #[serde(default)]
        lines: Vec<QobuzLyricsLine>,
    },
    /// Plain (non-synced) lyrics: text-only lines, no timestamps.
    #[serde(rename = "plain")]
    Plain {
        #[serde(default)]
        lang: Option<String>,
        #[serde(default)]
        lines: Vec<QobuzLyricsPlainLine>,
    },
}

impl QobuzLyricsContent {
    /// True when this document carries per-line timestamps.
    pub fn is_synced(&self) -> bool {
        matches!(self, QobuzLyricsContent::Synced { .. })
    }

    /// Number of lines in the document.
    pub fn line_count(&self) -> usize {
        match self {
            QobuzLyricsContent::Synced { lines, .. } => lines.len(),
            QobuzLyricsContent::Plain { lines, .. } => lines.len(),
        }
    }

    /// ISO 639-1 of the content's language, when the document carries it.
    pub fn lang(&self) -> Option<&str> {
        match self {
            QobuzLyricsContent::Synced { lang, .. } => lang.as_deref(),
            QobuzLyricsContent::Plain { lang, .. } => lang.as_deref(),
        }
    }
}

/// One synced line.
///
/// Live divergence from the yaml's `LyricsLineDto` (`{line,start,end}` all
/// required): instrumental-gap separator lines come as `{"line":"","words":[]}`
/// with NO `start`/`end` at all — both must be `Option`. Regular lines carry
/// line-level `start`/`end` plus per-word stamps (lyrics-doc-wsync.json).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzLyricsLine {
    #[serde(default)]
    pub line: String,
    /// Per-word timestamps ("wsync"); empty for gap lines and "lsync" docs.
    #[serde(default)]
    pub words: Vec<QobuzLyricsWord>,
    /// Line start, ms from track start. Absent on gap lines.
    #[serde(default)]
    pub start: Option<i64>,
    /// Line end, ms from track start. Absent on gap lines.
    #[serde(default)]
    pub end: Option<i64>,
}

/// One word inside a synced line (live-only; not in the yaml).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzLyricsWord {
    #[serde(default)]
    pub word: String,
    /// Word start, ms from track start.
    #[serde(default)]
    pub start: i64,
    /// Word end, ms from track start.
    #[serde(default)]
    pub end: i64,
}

/// One plain line. Live wire form is an object `{"line": "..."}`
/// (lyrics-doc-plain.json); the yaml guessed bare strings (`:287-303`) —
/// accept both.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QobuzLyricsPlainLine {
    pub line: String,
}

impl<'de> Deserialize<'de> for QobuzLyricsPlainLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Object {
                #[serde(default)]
                line: String,
            },
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Text(line) => QobuzLyricsPlainLine { line },
            Repr::Object { line } => QobuzLyricsPlainLine { line },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures are excerpts of the live captures of 2026-06-10 (full
    // sanitized copies live in qbz-nix-docs/qobuz-api/lyrics-*.json; the
    // synced/plain docs are truncated to a few lines here to avoid vendoring
    // full copyrighted lyrics in the repo — the STRUCTURE is verbatim).
    const URL_RESPONSE: &str = include_str!("../tests/fixtures/lyrics-url-response.json");
    const URL_RESPONSE_TRANSLATION: &str =
        include_str!("../tests/fixtures/lyrics-url-response-translation.json");
    const DOC_WSYNC: &str = include_str!("../tests/fixtures/lyrics-doc-wsync.json");
    const DOC_PLAIN: &str = include_str!("../tests/fixtures/lyrics-doc-plain.json");
    const DOC_WSYNC_TRANSLATION: &str =
        include_str!("../tests/fixtures/lyrics-doc-wsync-translation.json");
    const DOC_LSYNC_TRANSLATION: &str =
        include_str!("../tests/fixtures/lyrics-doc-lsync-translation.json");
    const DOC_PLAIN_TRANSLATION: &str =
        include_str!("../tests/fixtures/lyrics-doc-plain-translation.json");
    const DOC_TRANSLATION_ONLY: &str =
        include_str!("../tests/fixtures/lyrics-doc-translation-only.json");
    const MISS_RESPONSE: &str = include_str!("../tests/fixtures/lyrics-miss-response.json");

    #[test]
    fn deserialize_live_lyrics_url_response() {
        let urls: QobuzLyricsUrls =
            serde_json::from_str(URL_RESPONSE).expect("parse step-1 envelope");
        // Live shape: numeric track_id normalized to string; album_id is a slug.
        assert_eq!(urls.track_id.as_deref(), Some("266725027"));
        assert_eq!(urls.album_id.as_deref(), Some("gvcirtodd95kc"));
        let url = urls.lyrics_url.expect("lyrics_url present");
        assert!(url.starts_with("https://"));
        assert!(urls.translation_requested.is_none());
    }

    #[test]
    fn deserialize_live_wsync_document() {
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_WSYNC).expect("parse wsync document");
        assert_eq!(doc.track_id.as_deref(), Some("266725027"));
        assert_eq!(doc.album_id.as_deref(), Some("gvcirtodd95kc"));
        assert!(!doc.translation_langs.is_empty());
        assert!(!doc.publishers.is_empty());
        assert!(doc.writers.is_some());

        let content = doc.original.expect("original content present");
        assert!(content.is_synced());
        match &content {
            QobuzLyricsContent::Synced { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("en"));
                assert_eq!(lines.len(), 3);
                // Regular line: text + line stamps + word stamps.
                let first = &lines[0];
                assert_eq!(first.line, "Oh, mm-mm");
                assert_eq!(first.start, Some(1750));
                assert_eq!(first.end, Some(4770));
                assert_eq!(first.words.len(), 2);
                assert_eq!(first.words[0].word, "Oh,");
                assert_eq!(first.words[0].start, 1750);
                assert_eq!(first.words[0].end, 2480);
                // Gap line: empty text, no words, NO start/end on the wire.
                let gap = &lines[1];
                assert!(gap.line.is_empty());
                assert!(gap.words.is_empty());
                assert!(gap.start.is_none());
                assert!(gap.end.is_none());
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn deserialize_live_plain_document() {
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_PLAIN).expect("parse plain document");
        assert_eq!(doc.track_id.as_deref(), Some("29006863"));
        let content = doc.original.expect("original content present");
        assert!(!content.is_synced());
        match &content {
            QobuzLyricsContent::Plain { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("en"));
                assert_eq!(lines.len(), 4);
                // Live plain lines are OBJECTS {"line": "..."}.
                assert_eq!(lines[0].line, "Night follows me when you're gone");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn miss_response_is_not_a_lyrics_envelope() {
        // Live miss = HTTP 404 with Qobuz's standard error body. The client
        // maps non-200 to Ok(None) before parsing, but make sure the error
        // body could never masquerade as a usable envelope.
        let parsed: Result<QobuzLyricsUrls, _> = serde_json::from_str(MISS_RESPONSE);
        if let Ok(urls) = parsed {
            assert!(urls.lyrics_url.is_none());
        }
    }

    #[test]
    fn lyrics_url_response_tolerates_string_ids() {
        // Yaml-shaped variant (track_id as string) must also parse — the wire
        // showed a number, the spec says string; accept both.
        let json = r#"{"track_id":"1","album_id":"2","lyrics_url":"https://example.com/l.json"}"#;
        let urls: QobuzLyricsUrls = serde_json::from_str(json).expect("parse string-id variant");
        assert_eq!(urls.track_id.as_deref(), Some("1"));
        assert_eq!(urls.album_id.as_deref(), Some("2"));
    }

    #[test]
    fn deserialize_lyrics_url_response_with_translation_requested() {
        // v10: when a `language` was requested and a translation exists, the
        // envelope carries the `translation_requested` pointer (yaml
        // `LyricsTranslationDto`).
        let urls: QobuzLyricsUrls =
            serde_json::from_str(URL_RESPONSE_TRANSLATION).expect("parse translation envelope");
        assert_eq!(urls.track_id.as_deref(), Some("266725027"));
        let requested = urls
            .translation_requested
            .expect("translation_requested present");
        assert_eq!(requested.lang.as_deref(), Some("es"));
        assert!(requested
            .lyrics_url
            .as_deref()
            .unwrap_or("")
            .starts_with("https://"));
    }

    #[test]
    fn deserialize_document_with_wsync_translation() {
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_WSYNC_TRANSLATION).expect("parse wsync+translation document");
        assert_eq!(doc.translation_langs.len(), 5);

        let original = doc.original.expect("original present");
        assert!(original.is_synced());
        assert_eq!(original.lang(), Some("en"));

        let translation = doc.translation.expect("embedded translation present");
        assert!(translation.is_synced());
        assert_eq!(translation.lang(), Some("es"));
        match &translation {
            QobuzLyricsContent::Synced { lines, .. } => {
                assert_eq!(lines.len(), 3);
                assert_eq!(lines[0].line, "Primera linea traducida");
                assert_eq!(lines[0].words.len(), 3);
                assert_eq!(lines[0].words[2].word, "traducida");
                assert!(lines[1].start.is_none(), "gap line without stamps");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn deserialize_document_with_lsync_translation() {
        // Line-synced variant: lsync alias on BOTH original and translation,
        // no per-word stamps.
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_LSYNC_TRANSLATION).expect("parse lsync+translation document");
        let original = doc.original.expect("original present");
        assert!(original.is_synced());
        let translation = doc.translation.expect("translation present");
        match &translation {
            QobuzLyricsContent::Synced { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("fr"));
                assert_eq!(lines.len(), 2);
                assert!(lines[0].words.is_empty());
                assert_eq!(lines[1].start, Some(5000));
            }
            _ => panic!("expected synced translation"),
        }
    }

    #[test]
    fn deserialize_document_with_plain_translation() {
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_PLAIN_TRANSLATION).expect("parse plain+translation document");
        let original = doc.original.expect("original present");
        assert!(!original.is_synced());
        let translation = doc.translation.expect("translation present");
        match &translation {
            QobuzLyricsContent::Plain { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("de"));
                assert_eq!(lines.len(), 2);
                assert_eq!(lines[0].line, "Einfache Uebersetzung eins");
            }
            _ => panic!("expected plain translation"),
        }
    }

    #[test]
    fn document_without_translation_stays_none() {
        // 9.x back-compat: existing captures have no `translation` member.
        for fixture in [DOC_WSYNC, DOC_PLAIN] {
            let doc: QobuzLyricsDocument =
                serde_json::from_str(fixture).expect("parse 9.x document");
            assert!(doc.translation.is_none());
        }
    }

    #[test]
    fn empty_translation_langs_means_feature_off() {
        // Feature-off document: no translations offered at all.
        let json = r#"{"track_id": 1, "translation_langs": [],
            "original": {"type": "plain", "lang": "en", "lines": [{"line": "hi"}]}}"#;
        let doc: QobuzLyricsDocument = serde_json::from_str(json).expect("parse");
        assert!(doc.translation_langs.is_empty());
        assert!(doc.translation.is_none());

        // Missing member entirely behaves the same.
        let json = r#"{"track_id": 1,
            "original": {"type": "plain", "lang": "en", "lines": [{"line": "hi"}]}}"#;
        let doc: QobuzLyricsDocument = serde_json::from_str(json).expect("parse");
        assert!(doc.translation_langs.is_empty());
    }

    #[test]
    fn deserialize_translation_only_document() {
        // v10 LIVE WIRE (verified 2026-07-22, track 266725027 with
        // language=es): a translation request returns a document carrying
        // ONLY `translation` — no `original` at all.
        let doc: QobuzLyricsDocument = serde_json::from_str(DOC_TRANSLATION_ONLY)
            .expect("parse translation-only document");
        assert!(doc.original.is_none(), "no original on the translated doc");
        assert_eq!(doc.translation_langs.len(), 5);
        let translation = doc.translation.expect("translation present");
        assert!(translation.is_synced(), "lsync alias maps to Synced");
        assert_eq!(translation.lang(), Some("es"));
        match &translation {
            QobuzLyricsContent::Synced { lines, .. } => {
                assert_eq!(lines.len(), 4);
                assert!(lines[1].start.is_none(), "gap line without stamps");
                assert_eq!(lines[2].line, "Podría comerme a esa chica para el almuerzo");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn merge_translation_into_original_document() {
        // The get_lyrics merge path: original doc (9.x shape) + translation-
        // only doc combine into one bilingual document.
        let mut original: QobuzLyricsDocument =
            serde_json::from_str(DOC_WSYNC).expect("parse original document");
        assert!(original.translation.is_none());
        let translated: QobuzLyricsDocument =
            serde_json::from_str(DOC_TRANSLATION_ONLY).expect("parse translation-only document");

        merge_translation_into(&mut original, translated);

        assert!(original.original.is_some(), "original content kept");
        let translation = original.translation.expect("merged translation");
        assert_eq!(translation.lang(), Some("es"));
        // translation_langs unioned WITHOUT duplicates (both fixtures carry
        // the same 5 codes).
        assert_eq!(original.translation_langs.len(), 5);
        let mut sorted = original.translation_langs.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), original.translation_langs.len());
        // Writers/publishers stay the original document's.
        assert!(original.writers.is_some());
    }

    #[test]
    fn content_serializes_back_to_wire_shape() {
        // The lyrics cache persists an embedded translation verbatim, so the
        // content union must round-trip through serde.
        let doc: QobuzLyricsDocument =
            serde_json::from_str(DOC_WSYNC_TRANSLATION).expect("parse");
        let translation = doc.translation.expect("translation present");
        let json = serde_json::to_string(&translation).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "wsync");
        assert_eq!(value["lang"], "es");
        assert_eq!(value["lines"][0]["words"][0]["word"], "Primera");
        let back: QobuzLyricsContent = serde_json::from_str(&json).expect("re-parse");
        assert_eq!(back.lang(), Some("es"));
        assert_eq!(back.line_count(), 3);
    }

    #[test]
    fn lsync_alias_parses_yaml_shape() {
        // The yaml's "lsync" guess ({line,start,end}, no words) must still
        // parse via the alias in case line-only docs exist in the catalog.
        let json = r#"{"type":"lsync","lang":"fr","lines":[{"line":"hi","start":100,"end":200}]}"#;
        let content: QobuzLyricsContent =
            serde_json::from_str(json).expect("parse lsync alias");
        match content {
            QobuzLyricsContent::Synced { lang, lines } => {
                assert_eq!(lang.as_deref(), Some("fr"));
                assert_eq!(lines.len(), 1);
                assert_eq!(lines[0].start, Some(100));
                assert!(lines[0].words.is_empty());
            }
            _ => panic!("expected synced"),
        }
    }

    #[test]
    fn plain_lines_tolerate_bare_strings() {
        // Yaml-shaped plain variant (lines as bare strings) must also parse.
        let json = r#"{"type":"plain","lines":["first line","second line"]}"#;
        let content: QobuzLyricsContent =
            serde_json::from_str(json).expect("parse string-lines plain");
        assert_eq!(content.line_count(), 2);
        match content {
            QobuzLyricsContent::Plain { lines, .. } => {
                assert_eq!(lines[0].line, "first line");
            }
            _ => panic!("expected plain"),
        }
    }
}
