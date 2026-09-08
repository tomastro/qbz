//! Qobuz word-synced (wsync) document — the persistable native form.
//!
//! Amended Q5 (spec Addendum 2): the cache keeps the shared `synced_lrc`
//! column (Tauri-compat, LRC-with-gap-markers emission — lossy at word level
//! only) AND an additive `qobuz_wsync_json` column holding this document.
//! Readers prefer wsync when present.
//!
//! The serialized shape mirrors the Qobuz wire content union exactly
//! (`{"type":"wsync","lang":…,"lines":[{"line","start","end","words":[{"word","start","end"}]}]}`,
//! live captures `qbz-nix-docs/qobuz-api/lyrics-doc-wsync.json`), so a wire
//! content union parses as a [`QobuzWsync`] and a stored [`QobuzWsync`] is
//! wire-shaped. The Qobuz wrapper metadata (`translation_langs`, `writers`)
//! and the v10 `translation` block are carried as additive optional fields
//! so they survive the cache.

use serde::{Deserialize, Serialize};

use qbz_qobuz::{QobuzLyricsContent, QobuzLyricsDocument};

use crate::lrc::{emit_lrc, LrcSpan};
use crate::model::{
    derive_has_translation, LyricsDoc, LyricsKind, LyricsLine, LyricsProvider, TranslatedLyrics,
    Word,
};

/// Discriminator tag — always `"wsync"` on output; accepts `"lsync"` on input
/// (the yaml's guess, kept as alias like the wire DTO does) and defaults when
/// absent (stored docs are self-describing but tolerant).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum WsyncTag {
    #[default]
    #[serde(rename = "wsync", alias = "lsync")]
    Wsync,
}

/// Native word-synced lyrics document (cache column `qobuz_wsync_json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzWsync {
    #[serde(rename = "type", default)]
    pub tag: WsyncTag,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub lines: Vec<QobuzWsyncLine>,
    /// Wrapper metadata passthrough (additive; absent on wire content unions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub translation_langs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writers: Option<String>,
    /// Embedded translation (API v10), kept in the wire content-union shape
    /// so it persists verbatim inside the cache JSON; the content's own
    /// `lang` records which language the translation is for (a request for a
    /// DIFFERENT language refetches). `serde(default)` — no DB migration:
    /// rows written before this member existed still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation: Option<QobuzLyricsContent>,
}

/// One synced line, wire-shaped. Gap separator lines come as
/// `{"line":"","words":[]}` with no `start`/`end` (live divergence — see
/// `qbz-qobuz/src/lyrics.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QobuzWsyncLine {
    #[serde(default)]
    pub line: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<QobuzWsyncWord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
}

/// One word stamp, wire-shaped (`word`/`start`/`end`, ms from track start).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QobuzWsyncWord {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub start: i64,
    #[serde(default)]
    pub end: i64,
}

impl QobuzWsync {
    /// Build from a fetched Qobuz document. `None` unless the document's
    /// content is the synced variant. ALL lines are preserved verbatim
    /// (including empty gap separators) — this is the native form.
    pub fn from_document(doc: &QobuzLyricsDocument) -> Option<Self> {
        match doc.original.as_ref()? {
            QobuzLyricsContent::Synced { lang, lines } => Some(Self {
                tag: WsyncTag::Wsync,
                lang: lang.clone(),
                lines: lines
                    .iter()
                    .map(|line| QobuzWsyncLine {
                        line: line.line.clone(),
                        words: line
                            .words
                            .iter()
                            .map(|word| QobuzWsyncWord {
                                word: word.word.clone(),
                                start: word.start,
                                end: word.end,
                            })
                            .collect(),
                        start: line.start,
                        end: line.end,
                    })
                    .collect(),
                translation_langs: doc.translation_langs.clone(),
                writers: doc.writers.clone(),
                translation: doc.translation.clone(),
            }),
            QobuzLyricsContent::Plain { .. } => None,
        }
    }

    /// Convert to the domain model. Gap separator lines (empty text) and
    /// unplaceable lines (text without a `start` stamp) are skipped — in
    /// wsync the previous line already carries its explicit `end`, so nothing
    /// is lost. Native word stamps are preserved. `requested_lang` (the
    /// active translation target, if any) drives the client-derived
    /// `has_translation`; the embedded translation maps like the original.
    pub fn to_doc(&self, requested_lang: Option<&str>) -> LyricsDoc {
        let lines: Vec<LyricsLine> = self
            .lines
            .iter()
            .filter(|line| !line.line.trim().is_empty())
            .filter_map(|line| {
                let start = line.start?;
                Some(LyricsLine {
                    time_ms: Some(start),
                    end_ms: line.end,
                    text: line.line.clone(),
                    words: if line.words.is_empty() {
                        None
                    } else {
                        Some(
                            line.words
                                .iter()
                                .map(|word| Word {
                                    start: word.start,
                                    end: word.end,
                                    text: word.word.clone(),
                                })
                                .collect(),
                        )
                    },
                })
            })
            .collect();
        LyricsDoc {
            synced: !lines.is_empty(),
            lines,
            provider: LyricsProvider::Qobuz,
            translation_langs: self.translation_langs.clone(),
            writers: self.writers.clone(),
            translation: self
                .translation
                .as_ref()
                .map(|content| Box::new(translated_from_content(content))),
            has_translation: derive_has_translation(&self.translation_langs, requested_lang),
        }
    }

    /// Emit the cross-frontend LRC form (gap markers carry end-of-vocal;
    /// lossy at word level only — documented Q5 trade-off).
    pub fn to_lrc(&self) -> String {
        let spans: Vec<LrcSpan> = self
            .lines
            .iter()
            .filter(|line| !line.line.trim().is_empty())
            .filter_map(|line| {
                let start_ms = line.start?;
                Some(LrcSpan {
                    start_ms,
                    end_ms: line.end,
                    text: line.line.clone(),
                })
            })
            .collect();
        emit_lrc(&spans)
    }
}

/// Map a wire content union to the domain [`TranslatedLyrics`] — used for a
/// document's embedded `translation` block. Kind dispatch mirrors the
/// Android v10 mapper (`qt6.java`): wsync (any word stamps) -> WordSynced,
/// lsync (synced, no words) -> LineSynced, plain -> Plain. Synced lines keep
/// their stamps (the SAME timings as the original) and are filtered exactly
/// like `to_doc`, so translation lines stay 1:1 aligned with the rendered
/// original lines.
pub(crate) fn translated_from_content(content: &QobuzLyricsContent) -> TranslatedLyrics {
    match content {
        QobuzLyricsContent::Synced { lang, lines } => {
            let mapped: Vec<LyricsLine> = lines
                .iter()
                .filter(|line| !line.line.trim().is_empty())
                .filter_map(|line| {
                    let start = line.start?;
                    Some(LyricsLine {
                        time_ms: Some(start),
                        end_ms: line.end,
                        text: line.line.clone(),
                        words: if line.words.is_empty() {
                            None
                        } else {
                            Some(
                                line.words
                                    .iter()
                                    .map(|word| Word {
                                        start: word.start,
                                        end: word.end,
                                        text: word.word.clone(),
                                    })
                                    .collect(),
                            )
                        },
                    })
                })
                .collect();
            let kind = if mapped.iter().any(|line| line.words.is_some()) {
                LyricsKind::WordSynced
            } else {
                LyricsKind::LineSynced
            };
            TranslatedLyrics {
                kind,
                lang: lang.clone(),
                lines: mapped,
            }
        }
        QobuzLyricsContent::Plain { lang, lines } => TranslatedLyrics {
            kind: LyricsKind::Plain,
            lang: lang.clone(),
            lines: lines
                .iter()
                .map(|line| LyricsLine {
                    time_ms: None,
                    end_ms: None,
                    text: line.line.clone(),
                    words: None,
                })
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> QobuzLyricsDocument {
        // Shape mirrors the live capture lyrics-doc-wsync.json (LUNCH /
        // Billie Eilish): regular lines with word stamps + a gap separator
        // line without stamps.
        let json = r#"{
            "album_id": "gvcirtodd95kc",
            "track_id": 266725027,
            "translation_langs": ["es", "fr"],
            "publishers": [{"copyright": "(c) Example", "zones": ["WW"]}],
            "writers": "Billie Eilish O'Connell",
            "original": {
                "type": "wsync",
                "lang": "en",
                "lines": [
                    {"line": "Oh, mm-mm", "start": 1750, "end": 4770,
                     "words": [
                        {"word": "Oh,", "start": 1750, "end": 2480},
                        {"word": "mm-mm", "start": 2480, "end": 4770}
                     ]},
                    {"line": "", "words": []},
                    {"line": "I could eat that girl for lunch", "start": 19690, "end": 22180,
                     "words": [
                        {"word": "I", "start": 19690, "end": 19890},
                        {"word": "could", "start": 19890, "end": 20180},
                        {"word": "eat", "start": 20180, "end": 20560},
                        {"word": "that", "start": 20560, "end": 20850},
                        {"word": "girl", "start": 20850, "end": 21310},
                        {"word": "for", "start": 21310, "end": 21600},
                        {"word": "lunch", "start": 21600, "end": 22180}
                     ]}
                ]
            }
        }"#;
        serde_json::from_str(json).expect("valid document fixture")
    }

    #[test]
    fn from_document_preserves_lines_words_and_metadata() {
        let wsync = QobuzWsync::from_document(&sample_document()).expect("synced doc");
        assert_eq!(wsync.lang.as_deref(), Some("en"));
        assert_eq!(wsync.lines.len(), 3); // gap separator preserved natively
        assert_eq!(wsync.lines[0].words.len(), 2);
        assert_eq!(wsync.translation_langs, vec!["es", "fr"]);
        assert_eq!(wsync.writers.as_deref(), Some("Billie Eilish O'Connell"));
    }

    #[test]
    fn from_document_is_none_for_plain() {
        let json = r#"{
            "track_id": 29006863,
            "original": {"type": "plain", "lang": "en", "lines": [{"line": "Night"}]}
        }"#;
        let doc: QobuzLyricsDocument = serde_json::from_str(json).unwrap();
        assert!(QobuzWsync::from_document(&doc).is_none());
    }

    #[test]
    fn to_doc_skips_gaps_and_preserves_words() {
        let wsync = QobuzWsync::from_document(&sample_document()).unwrap();
        let doc = wsync.to_doc(None);
        assert!(doc.synced);
        assert_eq!(doc.provider, LyricsProvider::Qobuz);
        assert_eq!(doc.lines.len(), 2); // gap separator dropped from render model
        assert_eq!(doc.lines[0].time_ms, Some(1750));
        assert_eq!(doc.lines[0].end_ms, Some(4770));
        let words = doc.lines[1].words.as_ref().expect("words preserved");
        assert_eq!(words.len(), 7);
        assert_eq!(words[6].text, "lunch");
        assert_eq!(words[6].start, 21_600);
        assert_eq!(doc.translation_langs, vec!["es", "fr"]);
        assert_eq!(doc.writers.as_deref(), Some("Billie Eilish O'Connell"));
    }

    #[test]
    fn to_lrc_emits_gap_markers_from_native_ends() {
        let wsync = QobuzWsync::from_document(&sample_document()).unwrap();
        let lrc = wsync.to_lrc();
        // Vocal ends at 4770, next line starts at 19690 -> gap marker.
        assert!(lrc.contains("[00:01.750] Oh, mm-mm"));
        assert!(lrc.contains("[00:04.770]\n"));
        assert!(lrc.contains("[00:19.690] I could eat that girl for lunch"));
        // wsync -> model -> LRC -> parse: bounds recoverable by the parser.
        let parsed = crate::lrc::parse_lrc(&lrc);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].end_ms, Some(4770));
        assert_eq!(parsed[1].end_ms, Some(22_180));
        // …and word stamps are gone in the LRC form (documented lossiness).
        assert!(parsed.iter().all(|line| line.words.is_none()));
    }

    #[test]
    fn stored_json_round_trips_and_stays_wire_shaped() {
        let wsync = QobuzWsync::from_document(&sample_document()).unwrap();
        let json = serde_json::to_string(&wsync).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["type"], "wsync");
        assert_eq!(value["lines"][0]["words"][0]["word"], "Oh,");
        let back: QobuzWsync = serde_json::from_str(&json).unwrap();
        assert_eq!(back, wsync);

        // A bare wire content union (no metadata, as fetched from the CDN)
        // parses too.
        let wire = r#"{"type":"wsync","lang":"en","lines":[{"line":"hi","start":1,"end":2,
            "words":[{"word":"hi","start":1,"end":2}]}]}"#;
        let parsed: QobuzWsync = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.lines.len(), 1);
        assert!(parsed.translation_langs.is_empty());

        // The yaml-era "lsync" tag is tolerated on input.
        let lsync = r#"{"type":"lsync","lines":[{"line":"hi","start":1,"end":2}]}"#;
        let parsed: QobuzWsync = serde_json::from_str(lsync).unwrap();
        assert_eq!(parsed.lines[0].start, Some(1));
    }

    fn sample_document_with_translation() -> QobuzLyricsDocument {
        // v10 shape: the document embeds a `translation` block (same content
        // union as `original`) when a `language` was requested.
        let json = r#"{
            "album_id": "gvcirtodd95kc",
            "track_id": 266725027,
            "translation_langs": ["pt", "de", "fr", "es", "it"],
            "writers": "Billie Eilish O'Connell",
            "original": {
                "type": "wsync",
                "lang": "en",
                "lines": [
                    {"line": "Oh, mm-mm", "start": 1750, "end": 4770,
                     "words": [
                        {"word": "Oh,", "start": 1750, "end": 2480},
                        {"word": "mm-mm", "start": 2480, "end": 4770}
                     ]}
                ]
            },
            "translation": {
                "type": "wsync",
                "lang": "es",
                "lines": [
                    {"line": "Oh, mm-mm (es)", "start": 1750, "end": 4770,
                     "words": [
                        {"word": "Oh,", "start": 1750, "end": 2480},
                        {"word": "mm-mm", "start": 2480, "end": 4770}
                     ]}
                ]
            }
        }"#;
        serde_json::from_str(json).expect("valid translation fixture")
    }

    #[test]
    fn from_document_carries_the_embedded_translation() {
        let wsync =
            QobuzWsync::from_document(&sample_document_with_translation()).expect("synced doc");
        let translation = wsync.translation.expect("translation preserved");
        assert_eq!(translation.lang(), Some("es"));
        assert_eq!(translation.line_count(), 1);

        // 9.x documents (no `translation` member) keep `translation: None`.
        let wsync = QobuzWsync::from_document(&sample_document()).expect("synced doc");
        assert!(wsync.translation.is_none());
    }

    #[test]
    fn to_doc_maps_translation_and_derives_has_translation() {
        let wsync = QobuzWsync::from_document(&sample_document_with_translation()).unwrap();

        // Requested lang listed -> has_translation; translation mapped with
        // kind dispatch (wsync -> WordSynced) and native word stamps.
        let doc = wsync.to_doc(Some("es"));
        assert!(doc.has_translation);
        let translation = doc.translation.expect("translation mapped");
        assert_eq!(translation.kind, LyricsKind::WordSynced);
        assert_eq!(translation.lang.as_deref(), Some("es"));
        assert_eq!(translation.lines.len(), 1);
        assert_eq!(translation.lines[0].time_ms, Some(1750));
        let words = translation.lines[0].words.as_ref().expect("words survive");
        assert_eq!(words[1].text, "mm-mm");
        assert_eq!(words[1].start, 2480);

        // Requested lang NOT in translation_langs -> no has_translation (the
        // mapped block may still be present; the caller renders original-only).
        let doc = wsync.to_doc(Some("ja"));
        assert!(!doc.has_translation);

        // No language requested -> has_translation false by construction.
        let doc = wsync.to_doc(None);
        assert!(!doc.has_translation);
        assert!(doc.translation.is_some());
    }

    #[test]
    fn translation_kind_dispatch_covers_lsync_and_plain() {
        // lsync: synced without words -> LineSynced.
        let content: QobuzLyricsContent = serde_json::from_str(
            r#"{"type":"lsync","lang":"fr","lines":[{"line":"salut","start":100,"end":200}]}"#,
        )
        .unwrap();
        let translated = translated_from_content(&content);
        assert_eq!(translated.kind, LyricsKind::LineSynced);
        assert_eq!(translated.lines[0].time_ms, Some(100));
        assert!(translated.lines[0].words.is_none());

        // plain: text-only lines -> Plain, no stamps.
        let content: QobuzLyricsContent = serde_json::from_str(
            r#"{"type":"plain","lang":"de","lines":[{"line":"hallo"},{"line":"welt"}]}"#,
        )
        .unwrap();
        let translated = translated_from_content(&content);
        assert_eq!(translated.kind, LyricsKind::Plain);
        assert_eq!(translated.lines.len(), 2);
        assert_eq!(translated.lines[0].time_ms, None);
    }

    #[test]
    fn stored_json_with_translation_round_trips_and_old_rows_still_load() {
        let wsync = QobuzWsync::from_document(&sample_document_with_translation()).unwrap();
        let json = serde_json::to_string(&wsync).unwrap();
        let back: QobuzWsync = serde_json::from_str(&json).unwrap();
        assert_eq!(back, wsync);
        assert_eq!(
            back.translation.as_ref().and_then(|t| t.lang().map(str::to_owned)),
            Some("es".to_string())
        );

        // NO DB migration: a row written BEFORE the translation member
        // existed must still deserialize (serde default -> None).
        let old_format = r#"{"type":"wsync","lang":"en","translation_langs":["es","fr"],
            "lines":[{"line":"hi","start":1,"end":2,
            "words":[{"word":"hi","start":1,"end":2}]}]}"#;
        let parsed: QobuzWsync = serde_json::from_str(old_format).unwrap();
        assert!(parsed.translation.is_none());
        let doc = parsed.to_doc(Some("es"));
        assert!(doc.translation.is_none());
        assert!(doc.has_translation, "lang listed -> has_translation");
    }
}
