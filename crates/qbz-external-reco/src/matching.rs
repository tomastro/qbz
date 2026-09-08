//! Text + ISRC scoring to validate a recommendation against the Qobuz catalog.
//!
//! Clean-room port of `qbz-playlist-import::match_qobuz` (the proven importer
//! scorer), adapted to score a lightweight [`MatchInput`] (artist/title/album/
//! duration/isrc) against a `qbz_models::Track`. Kept self-contained here so the
//! engine has no dependency on the importer crate; the algorithm is identical
//! (ISRC short-circuit -> title*0.6 + artist*0.3 + album*0.1 + duration bonus,
//! `!is_streamable()` skipped, hi-res tiebreak).

use qbz_models::Track;

const TITLE_WEIGHT: f32 = 0.6;
const ARTIST_WEIGHT: f32 = 0.3;
const ALBUM_WEIGHT: f32 = 0.1;

/// Minimum score to accept a text-only (no-ISRC) match. Mirrors the importer.
pub const MIN_SCORE: f32 = 0.65;

/// The recommendation being matched against Qobuz candidates.
pub struct MatchInput<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: Option<&'a str>,
    pub duration_ms: Option<u64>,
    pub isrc: Option<&'a str>,
}

/// Pick the best streamable candidate and its score (hi-res breaks ties).
/// Does NOT gate on [`MIN_SCORE`] — the caller decides.
pub fn select_best_match<'a>(input: &MatchInput, candidates: &'a [Track]) -> (Option<&'a Track>, f32) {
    let mut best: Option<&Track> = None;
    let mut best_score = 0.0f32;
    let mut best_quality = 0.0f32;

    for candidate in candidates {
        // Only an explicit `streamable: false` disqualifies; an endpoint that
        // omits the key must not silently empty the candidate set. Kept
        // identical to the importer's scorer, which this file mirrors.
        if !candidate.is_streamable() {
            continue;
        }
        let score = score_candidate(input, candidate);
        let quality = quality_score(candidate);

        if score > best_score + 0.0001 {
            best = Some(candidate);
            best_score = score;
            best_quality = quality;
        } else if (score - best_score).abs() < 0.01 && quality > best_quality {
            best = Some(candidate);
            best_quality = quality;
        }
    }

    (best, best_score)
}

pub fn score_candidate(input: &MatchInput, candidate: &Track) -> f32 {
    if let (Some(isrc), Some(candidate_isrc)) = (input.isrc, &candidate.isrc) {
        if isrc.eq_ignore_ascii_case(candidate_isrc) {
            return 1.0;
        }
    }

    let title_score = similarity(input.title, &candidate.title);
    let artist_score = similarity(
        input.artist,
        candidate
            .performer
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or(""),
    );
    let album_score = input
        .album
        .map(|album| {
            candidate
                .album
                .as_ref()
                .map(|a| similarity(album, &a.title))
                .unwrap_or(0.0)
        })
        .unwrap_or(0.0);

    let mut score =
        title_score * TITLE_WEIGHT + artist_score * ARTIST_WEIGHT + album_score * ALBUM_WEIGHT;

    if let (Some(import_duration), candidate_duration) = (
        input.duration_ms,
        (candidate.duration as u64).saturating_mul(1000),
    ) {
        let diff = if import_duration > candidate_duration {
            import_duration - candidate_duration
        } else {
            candidate_duration - import_duration
        };
        if diff <= 3000 {
            score += 0.05;
        } else if diff <= 5000 {
            score += 0.02;
        }
    }

    score
}

pub fn similarity(a: &str, b: &str) -> f32 {
    let na = normalize(a);
    let nb = normalize(b);
    if na.is_empty() || nb.is_empty() {
        return 0.0;
    }
    if na == nb {
        return 1.0;
    }
    if na.contains(&nb) || nb.contains(&na) {
        return 0.85;
    }
    token_overlap(&na, &nb)
}

/// Normalize a name for matching AND for cache keys (lowercase, strip brackets +
/// stop-words, collapse punctuation to spaces).
pub fn normalize(input: &str) -> String {
    let stripped = remove_bracketed(input);
    let mut cleaned = String::new();
    for ch in stripped.chars() {
        if ch.is_ascii_alphanumeric() || ch.is_whitespace() {
            cleaned.push(ch.to_ascii_lowercase());
        } else {
            cleaned.push(' ');
        }
    }
    let stop_words = [
        "remaster",
        "remastered",
        "deluxe",
        "edition",
        "live",
        "feat",
        "featuring",
        "version",
        "mix",
        "mono",
        "stereo",
        "edit",
    ];
    cleaned
        .split_whitespace()
        .filter(|token| !stop_words.contains(token))
        .collect::<Vec<_>>()
        .join(" ")
}

fn remove_bracketed(input: &str) -> String {
    let mut out = String::new();
    let mut depth = 0u32;
    for ch in input.chars() {
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {
                if depth == 0 {
                    out.push(ch);
                }
            }
        }
    }
    out
}

fn token_overlap(a: &str, b: &str) -> f32 {
    let a_tokens: Vec<&str> = a.split_whitespace().collect();
    let b_tokens: Vec<&str> = b.split_whitespace().collect();
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let mut matches = 0u32;
    for token in &a_tokens {
        if b_tokens.contains(token) {
            matches += 1;
        }
    }
    matches as f32 / a_tokens.len().max(b_tokens.len()) as f32
}

fn quality_score(track: &Track) -> f32 {
    let bit_depth = track.maximum_bit_depth.unwrap_or(0) as f32;
    let sample_rate = track.maximum_sampling_rate.unwrap_or(0.0) as f32;
    bit_depth * 100000.0 + sample_rate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_brackets_and_stop_words() {
        assert_eq!(normalize("Song Title (Remastered 2011)"), "song title");
        assert_eq!(normalize("Hey Jude - Remastered"), "hey jude");
    }

    #[test]
    fn similarity_exact_and_substring() {
        assert_eq!(similarity("Hey Jude (Remastered)", "hey jude"), 1.0);
        assert_eq!(similarity("hey jude", "hey jude na na"), 0.85);
        assert_eq!(similarity("", "anything"), 0.0);
    }

    #[test]
    fn token_overlap_uses_longer_side() {
        assert_eq!(token_overlap("a b", "a b c d"), 0.5);
        assert_eq!(token_overlap("x", "y"), 0.0);
    }

    #[test]
    fn normalize_is_stable_for_cache_keys() {
        assert_eq!(normalize("  The Beatles  "), "the beatles");
        assert_eq!(normalize("AC/DC"), "ac dc");
    }
}
