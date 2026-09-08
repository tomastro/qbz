//! LRC parser + emitter.
//!
//! The parser ports the TS semantics from `src/lib/stores/lyricsStore.ts:131-185`
//! verbatim: two-pass with empty-text gap markers as end-of-vocal bounds, the
//! `MAX_SUNG_MS = 8000` cap ("capping prevents the karaoke gradient from
//! creeping across silence"), and the median last-line end estimate.
//!
//! Fixed forward (defect F5, review §9.5):
//! - multi-timestamp lines (`[a][b]text`) are split — each stamp gets the
//!   text (the TS regex leaked the second stamp into the displayed text);
//! - `[offset:±ms]` is honored: positive offset advances the display
//!   (effective time = tag time − offset, the dominant player convention),
//!   clamped at 0.
//!
//! The emitter renders synced spans (Qobuz wsync line bounds) into
//! LRC-with-gap-markers — the persisted lingua franca both frontends already
//! speak (Q5): `[mm:ss.xxx] text` from `start`; an empty `[mm:ss.xxx]` gap
//! marker from `end` whenever the vocal ends before the next line starts
//! (and after the final line). Lossy at WORD level only — the native word
//! stamps persist separately in the `qobuz_wsync_json` column (amended Q5).

use regex::Regex;
use std::sync::OnceLock;

use crate::model::LyricsLine;

/// Cap on any single line's sung duration (ms) — parity with the TS parser
/// (`lyricsStore.ts:154`). Anything beyond this is almost certainly an
/// instrumental gap the LRC didn't mark.
pub const MAX_SUNG_MS: i64 = 8000;

/// One synced span for the emitter: line start, optional end-of-vocal, text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LrcSpan {
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub text: String,
}

fn stamp_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Same shape as the TS regex (`lyricsStore.ts:137`) minus the trailing
    // `(.*)`: text extraction is positional so consecutive stamps can be
    // collected (F5 multi-stamp fix). Supports [mm:ss.xx], [mm:ss.xxx],
    // [mm:ss], [mm:ss:xx].
    RE.get_or_init(|| Regex::new(r"\[(\d{1,2}):(\d{2})(?:[.:](\d{2,3}))?\]").expect("valid regex"))
}

fn offset_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*\[offset\s*:\s*([+-]?\d+)\s*\]").expect("valid regex")
    })
}

/// Parse an LRC blob into synced lines.
///
/// Two-pass: collect every timestamp (including empty-text gap markers like
/// `[02:34.00]` marking end-of-vocal before an instrumental break), then emit
/// only the text-bearing lines with each one's `end_ms` derived from the
/// following stamp (text OR gap), capped at `time_ms + MAX_SUNG_MS`.
pub fn parse_lrc(lrc: &str) -> Vec<LyricsLine> {
    let stamp_re = stamp_regex();
    let mut stamps: Vec<(i64, String)> = Vec::new();
    let mut offset_ms: i64 = 0;

    for raw_line in lrc.lines() {
        // [offset:] tag — honored, F5 (last tag wins; applied globally below).
        if let Some(caps) = offset_regex().captures(raw_line) {
            if let Ok(value) = caps[1].parse::<i64>() {
                offset_ms = value;
            }
            continue;
        }

        // Find the first stamp anywhere in the line (parity: the TS global
        // regex also matched mid-line), then collect CONSECUTIVE stamps from
        // there; the text after the last consecutive stamp applies to every
        // collected stamp (F5 multi-stamp split).
        let Some(first) = stamp_re.find(raw_line) else {
            continue; // metadata tags ([ar:], [ti:], …) and junk lines skipped
        };
        let mut times: Vec<i64> = Vec::new();
        let mut pos = first.start();
        while let Some(caps) = stamp_re.captures_at(raw_line, pos) {
            let whole = caps.get(0).expect("match 0");
            if whole.start() != pos {
                break; // non-adjacent stamp — belongs to the text
            }
            let minutes: i64 = caps[1].parse().unwrap_or(0);
            let seconds: i64 = caps[2].parse().unwrap_or(0);
            // Fractional part right-padded to ms ("50" -> 500ms), parity with
            // the TS `padEnd(3, '0')` (`lyricsStore.ts:143`).
            let ms: i64 = caps
                .get(3)
                .map(|frac| format!("{:0<3}", frac.as_str()).parse().unwrap_or(0))
                .unwrap_or(0);
            times.push((minutes * 60 + seconds) * 1000 + ms);
            pos = whole.end();
        }
        let text = raw_line[pos..].trim().to_string();
        for time_ms in times {
            stamps.push((time_ms, text.clone()));
        }
    }

    // Apply the offset to every stamp (positive = lyrics display earlier).
    if offset_ms != 0 {
        for stamp in &mut stamps {
            stamp.0 = (stamp.0 - offset_ms).max(0);
        }
    }

    // Stable sort keeps file order for equal timestamps (TS `Array.sort` on
    // pre-sorted input behaves the same for this data).
    stamps.sort_by_key(|stamp| stamp.0);

    let mut lines: Vec<LyricsLine> = Vec::new();
    for (i, stamp) in stamps.iter().enumerate() {
        if stamp.1.is_empty() {
            continue; // gap marker — never displayed, only bounds previous line
        }
        let cap = stamp.0 + MAX_SUNG_MS;
        let end_ms = stamps.get(i + 1).map(|next| next.0.min(cap));
        lines.push(LyricsLine {
            time_ms: Some(stamp.0),
            end_ms,
            text: stamp.1.clone(),
            words: None,
        });
    }

    // Last line has no following stamp: estimate from the median of the
    // preceding lines' sung durations, capped (`lyricsStore.ts:170-182`).
    if lines.len() >= 2 && lines.last().map(|l| l.end_ms.is_none()).unwrap_or(false) {
        let mut durations: Vec<i64> = Vec::new();
        for i in 0..lines.len() - 1 {
            let start = lines[i].time_ms.unwrap_or(0);
            let bound = lines[i]
                .end_ms
                .or_else(|| lines[i + 1].time_ms)
                .unwrap_or(start);
            let d = bound - start;
            if d > 0 {
                durations.push(d);
            }
        }
        if !durations.is_empty() {
            durations.sort_unstable();
            let median = durations[durations.len() / 2];
            let last = lines.last_mut().expect("len >= 2");
            last.end_ms = Some(last.time_ms.unwrap_or(0) + median.min(MAX_SUNG_MS));
        }
    }

    lines
}

/// Parse plain lyrics (no timestamps): split on `\n`, trim, drop empties
/// (parity: `parsePlain`, `lyricsStore.ts:190-196`; `time_ms` is `None`
/// instead of the TS `0` — the domain model encodes "unsynced" explicitly).
pub fn parse_plain(plain: &str) -> Vec<LyricsLine> {
    plain
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|text| LyricsLine {
            time_ms: None,
            end_ms: None,
            text: text.to_string(),
            words: None,
        })
        .collect()
}

fn format_stamp(ms: i64) -> String {
    // Tracks beyond 99:59.999 would emit a 3-digit minute field the parser's
    // `\d{1,2}` cannot read back — accepted edge (the native wsync column
    // carries the exact data for Qobuz documents anyway).
    let ms = ms.max(0);
    let minutes = ms / 60_000;
    let seconds = (ms % 60_000) / 1000;
    let millis = ms % 1000;
    format!("[{:02}:{:02}.{:03}]", minutes, seconds, millis)
}

/// Emit synced spans as LRC-with-gap-markers (Q5 persistence form).
///
/// - `[mm:ss.xxx] text` from each span's `start_ms`;
/// - an empty gap marker `[mm:ss.xxx]` from `end_ms` whenever the vocal ends
///   before the next span starts (real instrumental gap), and after the final
///   span — so the parser recovers the authoritative end-of-vocal for every
///   line (subject to its uniform `MAX_SUNG_MS` cap).
pub fn emit_lrc(spans: &[LrcSpan]) -> String {
    let mut out = String::new();
    for (i, span) in spans.iter().enumerate() {
        out.push_str(&format_stamp(span.start_ms));
        out.push(' ');
        out.push_str(span.text.trim());
        out.push('\n');

        if let Some(end_ms) = span.end_ms {
            if end_ms > span.start_ms {
                let needs_marker = match spans.get(i + 1) {
                    Some(next) => end_ms < next.start_ms,
                    None => true, // trailing marker bounds the last line exactly
                };
                if needs_marker {
                    out.push_str(&format_stamp(end_ms));
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(lines: &[LyricsLine], i: usize) -> (i64, Option<i64>, &str) {
        (
            lines[i].time_ms.expect("synced line"),
            lines[i].end_ms,
            lines[i].text.as_str(),
        )
    }

    #[test]
    fn parses_all_stamp_formats() {
        let lrc = "[00:01.50]a\n[00:03.500]b\n[00:05]c\n[00:07:25]d\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 4);
        assert_eq!(line(&lines, 0).0, 1500);
        assert_eq!(line(&lines, 1).0, 3500);
        assert_eq!(line(&lines, 2).0, 5000);
        assert_eq!(line(&lines, 3).0, 7250); // colon fraction [mm:ss:xx]
    }

    #[test]
    fn two_digit_fraction_pads_right_to_ms() {
        let lines = parse_lrc("[00:01.05]a\n[00:02.5]b\n");
        // ".05" -> 050ms. A single-digit fraction (".5") does NOT match
        // (\d{2,3}) so that stamp is no stamp at all and the line is skipped
        // — exactly what the TS regex did; keep parity.
        assert_eq!(lines.len(), 1);
        assert_eq!(line(&lines, 0).0, 1050);
    }

    #[test]
    fn gap_marker_bounds_previous_line() {
        let lrc = "[00:01.00]sung\n[00:03.00]\n[00:10.00]next\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2); // gap marker never displayed
        assert_eq!(line(&lines, 0), (1000, Some(3000), "sung"));
        assert_eq!(line(&lines, 1).2, "next");
    }

    #[test]
    fn end_capped_at_max_sung_ms() {
        let lrc = "[00:01.00]long line\n[00:20.00]later\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[0].end_ms, Some(1000 + MAX_SUNG_MS));
    }

    #[test]
    fn last_line_end_is_median_of_prior_durations_capped() {
        // Durations: 2000, 4000 -> sorted [2000, 4000], median idx 1 = 4000.
        let lrc = "[00:00.00]a\n[00:02.00]b\n[00:06.00]c\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[2].end_ms, Some(6000 + 4000));

        // Median above the cap clamps to MAX_SUNG_MS.
        let lrc = "[00:00.00]a\n[00:09.00]b\n[00:18.00]c\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[0].end_ms, Some(MAX_SUNG_MS)); // capped bound
        assert_eq!(lines[2].end_ms, Some(18_000 + MAX_SUNG_MS));
    }

    #[test]
    fn single_line_keeps_open_end() {
        let lines = parse_lrc("[00:01.00]only\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].end_ms, None); // TS guard: lines.length >= 2
    }

    #[test]
    fn multi_stamp_line_splits_per_stamp_f5() {
        let lrc = "[00:10.00][00:50.00]chorus\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 2);
        assert_eq!(line(&lines, 0), (10_000, Some(10_000 + MAX_SUNG_MS), "chorus"));
        assert_eq!(line(&lines, 1).0, 50_000);
        // No stamp leaks into the displayed text.
        assert!(!lines[0].text.contains('['));
    }

    #[test]
    fn offset_tag_honored_f5() {
        // Positive offset advances the display: effective = tag - offset.
        let lrc = "[offset:+500]\n[00:01.00]a\n[00:03.00]b\n";
        let lines = parse_lrc(lrc);
        assert_eq!(line(&lines, 0).0, 500);
        assert_eq!(line(&lines, 1).0, 2500);

        // Negative offset delays; clamped at zero.
        let lrc = "[offset:-250]\n[00:00.10]a\n[00:02.00]b\n";
        let lines = parse_lrc(lrc);
        assert_eq!(line(&lines, 0).0, 350);

        let lrc = "[offset:2000]\n[00:01.00]a\n[00:05.00]b\n";
        let lines = parse_lrc(lrc);
        assert_eq!(line(&lines, 0).0, 0); // 1000 - 2000 clamped
    }

    #[test]
    fn metadata_tags_skipped() {
        let lrc = "[ar:Artist]\n[ti:Title]\n[al:Album]\n[00:01.00]real\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "real");
    }

    #[test]
    fn stamps_sorted_by_time() {
        let lrc = "[00:05.00]second\n[00:01.00]first\n";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].text, "second");
        assert_eq!(lines[0].end_ms, Some(5000)); // bounded by the later stamp
    }

    #[test]
    fn parse_plain_trims_and_drops_empties() {
        let lines = parse_plain("a\n\n  b  \n\t\nc");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "b");
        assert!(lines.iter().all(|l| l.time_ms.is_none() && l.end_ms.is_none()));
    }

    #[test]
    fn emitter_writes_gap_markers_only_for_real_gaps() {
        let spans = vec![
            LrcSpan {
                start_ms: 1000,
                end_ms: Some(3000),
                text: "first".into(),
            },
            // contiguous: end == next start -> no marker
            LrcSpan {
                start_ms: 3000,
                end_ms: Some(5000),
                text: "second".into(),
            },
            // gap before this one (5000 < 9000) -> marker at 5000
            LrcSpan {
                start_ms: 9000,
                end_ms: Some(10_000),
                text: "third".into(),
            },
        ];
        let lrc = emit_lrc(&spans);
        let expected = "[00:01.000] first\n[00:03.000] second\n[00:05.000]\n[00:09.000] third\n[00:10.000]\n";
        assert_eq!(lrc, expected);
    }

    #[test]
    fn emit_parse_round_trip_preserves_bounds() {
        let spans = vec![
            LrcSpan {
                start_ms: 1750,
                end_ms: Some(4770),
                text: "Oh, mm-mm".into(),
            },
            LrcSpan {
                start_ms: 19_690,
                end_ms: Some(22_180),
                text: "Fly me to the moon".into(),
            },
        ];
        let lines = parse_lrc(&emit_lrc(&spans));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time_ms, Some(1750));
        assert_eq!(lines[0].end_ms, Some(4770)); // gap marker carried the end
        assert_eq!(lines[1].time_ms, Some(19_690));
        // Trailing marker bounds the LAST line with the exact end too.
        assert_eq!(lines[1].end_ms, Some(22_180));
    }

    #[test]
    fn round_trip_applies_uniform_cap_on_long_vocals() {
        // A single vocal sung >8s: the emitter writes the true end, the
        // parser caps it — the accepted uniform heuristic (Q5).
        let spans = vec![
            LrcSpan {
                start_ms: 0,
                end_ms: Some(12_000),
                text: "looooong".into(),
            },
            LrcSpan {
                start_ms: 15_000,
                end_ms: Some(16_000),
                text: "after".into(),
            },
        ];
        let lines = parse_lrc(&emit_lrc(&spans));
        assert_eq!(lines[0].end_ms, Some(MAX_SUNG_MS));
    }
}
