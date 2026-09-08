//! Sync-engine pure functions — the Rust port of the Tauri store's
//! active-line math (`lyricsStore.ts:221-264`), spec §4.2:
//!
//! - [`find_active_line_index`]: binary search over `time_ms` for the last
//!   line at-or-before the playback position; `-1` before the first stamp
//!   (`findActiveLineIndex`, `lyricsStore.ts:221-239`).
//! - [`line_progress`]: per-line progress 0→1 with the Tauri bound chain
//!   `end_ms ?? next.time_ms ?? time_ms + 5000`, clamped, **snapped to 1.0
//!   at ratio >= 0.99** (the anti-stuck-tail guard, `lyricsStore.ts:261`).
//! - [`line_fill_fraction`]: the karaoke clip fraction (Q2 final form) —
//!   **word-anchored** when the line carries native Qobuz wsync words,
//!   line-proportional (== `line_progress`) for LRC-sourced lines.
//!
//! All functions are pure over [`LyricsLine`] slices: headless-testable,
//! frontend-agnostic (ADR-006), shared by any future surface (miniplayer,
//! ImmersiveView, Tauri adoption).

use crate::model::LyricsLine;

/// Fallback duration for a line with no `end_ms` and no successor
/// (Tauri `calculateLineProgress`, `lyricsStore.ts:244-264`).
pub const DEFAULT_LINE_DURATION_MS: i64 = 5_000;

/// Progress ratio at/above which the value snaps to exactly 1.0 — guards the
/// visually stuck "0.2% tail" (`lyricsStore.ts:261`).
pub const PROGRESS_SNAP: f32 = 0.99;

fn snap(ratio: f32) -> f32 {
    if ratio >= PROGRESS_SNAP {
        1.0
    } else {
        ratio
    }
}

/// Index of the active line at `now_ms`: the LAST line whose `time_ms` is
/// `<= now_ms`; `-1` before the first stamp or for unstamped (plain) docs.
/// Binary search — `lines` must be stamp-ordered (parser/wsync guarantee);
/// missing stamps are treated as unreachable.
pub fn find_active_line_index(lines: &[LyricsLine], now_ms: i64) -> i32 {
    if lines.is_empty() {
        return -1;
    }
    let stamp = |i: usize| lines[i].time_ms.unwrap_or(i64::MAX);
    if now_ms < stamp(0) {
        return -1;
    }
    let mut lo = 0usize;
    let mut hi = lines.len();
    // Invariant: stamp(lo) <= now_ms < stamp(hi) (hi exclusive).
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if stamp(mid) <= now_ms {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo as i32
}

/// Time-proportional progress 0..=1 of `lines[index]` at `now_ms`.
/// Bound chain + snap per the Tauri reference (see module docs).
pub fn line_progress(lines: &[LyricsLine], index: usize, now_ms: i64) -> f32 {
    let Some(line) = lines.get(index) else {
        return 0.0;
    };
    let Some(start) = line.time_ms else {
        return 0.0;
    };
    let bound = line
        .end_ms
        .or_else(|| lines.get(index + 1).and_then(|next| next.time_ms))
        .unwrap_or(start + DEFAULT_LINE_DURATION_MS);
    if bound <= start {
        return 1.0;
    }
    let ratio = (now_ms - start) as f32 / (bound - start) as f32;
    snap(ratio.clamp(0.0, 1.0))
}

/// Karaoke clip fraction 0..=1 of the line's rendered width at `now_ms`
/// (drives the rect-clip overlay width — Q2 final form).
///
/// - **wsync path** (line has native `words`): the fraction interpolates
///   between REAL word boundaries. **Width approximation (documented):** a
///   word's pixel width is approximated by its character count's share of
///   the line's total characters — the engine has no glyph metrics, and the
///   per-char proportion is within a few percent for typical lyric text.
///   Each word except the last also absorbs one separator character, so the
///   fill sweeps the inter-word space while/after that word is sung; in the
///   silent gap between two words the fill holds at the end of the sung
///   word's span.
/// - **LRC path** (no words): degrades to the line-proportional
///   [`line_progress`] — exactly Tauri's behavior for external providers.
///
/// The uniform >=0.99 snap applies to the final fraction in both paths.
pub fn line_fill_fraction(lines: &[LyricsLine], index: usize, now_ms: i64) -> f32 {
    let Some(line) = lines.get(index) else {
        return 0.0;
    };
    let Some(words) = line.words.as_ref().filter(|words| !words.is_empty()) else {
        return line_progress(lines, index, now_ms);
    };

    // Char-proportional weights: word chars + 1 separator for all but last.
    let weights: Vec<f32> = words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let chars = word.text.chars().count() as f32;
            if i + 1 < words.len() {
                chars + 1.0
            } else {
                chars
            }
        })
        .collect();
    let total: f32 = weights.iter().sum();
    if total <= 0.0 {
        return line_progress(lines, index, now_ms);
    }

    if now_ms < words[0].start {
        return 0.0;
    }
    if now_ms >= words[words.len() - 1].end {
        return 1.0;
    }

    // Current word = last word whose start <= now (words are stamp-ordered).
    let mut current = 0usize;
    for (i, word) in words.iter().enumerate() {
        if word.start <= now_ms {
            current = i;
        } else {
            break;
        }
    }
    let word = &words[current];
    let span = word.end.saturating_sub(word.start);
    let word_progress = if span > 0 {
        ((now_ms - word.start) as f32 / span as f32).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let filled: f32 =
        weights[..current].iter().sum::<f32>() + weights[current] * word_progress;
    snap((filled / total).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Word;

    fn synced_line(time_ms: i64, end_ms: Option<i64>, text: &str) -> LyricsLine {
        LyricsLine {
            time_ms: Some(time_ms),
            end_ms,
            text: text.into(),
            words: None,
        }
    }

    fn ladder() -> Vec<LyricsLine> {
        vec![
            synced_line(1_000, Some(2_500), "one"),
            synced_line(3_000, None, "two"),
            synced_line(7_000, None, "three"),
        ]
    }

    #[test]
    fn active_index_binary_search_semantics() {
        let lines = ladder();
        assert_eq!(find_active_line_index(&[], 5_000), -1);
        assert_eq!(find_active_line_index(&lines, 0), -1); // before first stamp
        assert_eq!(find_active_line_index(&lines, 1_000), 0); // exactly at stamp
        assert_eq!(find_active_line_index(&lines, 2_999), 0);
        assert_eq!(find_active_line_index(&lines, 3_000), 1);
        assert_eq!(find_active_line_index(&lines, 999_999), 2); // after last

        // Plain (unstamped) docs never activate.
        let plain = vec![LyricsLine {
            time_ms: None,
            end_ms: None,
            text: "p".into(),
            words: None,
        }];
        assert_eq!(find_active_line_index(&plain, 5_000), -1);
    }

    #[test]
    fn progress_bound_chain_and_snap() {
        let lines = ladder();
        // end_ms bound: 1000..2500 → midpoint.
        assert!((line_progress(&lines, 0, 1_750) - 0.5).abs() < 1e-4);
        // No end_ms → next line's stamp: 3000..7000.
        assert!((line_progress(&lines, 1, 5_000) - 0.5).abs() < 1e-4);
        // Last line, no end_ms, no next → +5000 default: 7000..12000.
        assert!((line_progress(&lines, 2, 9_500) - 0.5).abs() < 1e-4);
        // Clamped to [0, 1].
        assert_eq!(line_progress(&lines, 0, 0), 0.0);
        assert_eq!(line_progress(&lines, 0, 99_999), 1.0);
        // Snap: >= 0.99 becomes exactly 1.0 (1000 + 0.995*1500 = 2492.5).
        assert_eq!(line_progress(&lines, 0, 2_493), 1.0);
        // Out-of-range index is inert.
        assert_eq!(line_progress(&lines, 9, 1_000), 0.0);
    }

    #[test]
    fn fill_fraction_lrc_path_equals_line_progress() {
        let lines = ladder();
        assert_eq!(
            line_fill_fraction(&lines, 0, 1_750),
            line_progress(&lines, 0, 1_750)
        );
    }

    #[test]
    fn fill_fraction_word_anchored() {
        // "ab cd": weights ab=2+1(sep)=3, cd=2; total 5.
        let line = LyricsLine {
            time_ms: Some(1_000),
            end_ms: Some(5_000),
            text: "ab cd".into(),
            words: Some(vec![
                Word {
                    start: 1_000,
                    end: 2_000,
                    text: "ab".into(),
                },
                Word {
                    start: 3_000,
                    end: 5_000,
                    text: "cd".into(),
                },
            ]),
        };
        let lines = vec![line];

        // Before the first word.
        assert_eq!(line_fill_fraction(&lines, 0, 500), 0.0);
        // Halfway through word 1: 3 * 0.5 / 5 = 0.3.
        assert!((line_fill_fraction(&lines, 0, 1_500) - 0.3).abs() < 1e-4);
        // In the gap between words: holds at word 1's full span, 3/5 = 0.6.
        assert!((line_fill_fraction(&lines, 0, 2_500) - 0.6).abs() < 1e-4);
        // Halfway through word 2: (3 + 2*0.5) / 5 = 0.8.
        assert!((line_fill_fraction(&lines, 0, 4_000) - 0.8).abs() < 1e-4);
        // At/after the last word's end.
        assert_eq!(line_fill_fraction(&lines, 0, 5_000), 1.0);
        // Snap near the end: (3 + 2*0.99)/5 = 0.996 >= 0.99 → 1.0.
        assert_eq!(line_fill_fraction(&lines, 0, 4_980), 1.0);
    }
}
