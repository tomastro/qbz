//! Minimal gettext `Plural-Forms` evaluator.
//!
//! We support the expressions our locales actually use:
//!   - `nplurals=2; plural=(n != 1);`  (en, es, de, pt, nl)
//!   - `nplurals=2; plural=(n > 1);`   (fr)
//!   - `nplurals=1; plural=0;`         (ja — no plural distinction)
//!   - `nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 &&
//!      (n%100<12 || n%100>14) ? 1 : 2);` (ru — Slavic one/few/many)
//! Anything unrecognized falls back to the English default `if n==1 {0} else {1}`.

/// The plural-selection kind extracted from a `Plural-Forms` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `plural=(n != 1)` — index 0 when n == 1, else 1.
    NotOne,
    /// `plural=(n > 1)` — index 0 when n <= 1, else 1.
    GreaterThanOne,
    /// `nplurals=1; plural=0` — a single form for every count (e.g. Japanese).
    Single,
    /// Slavic three-form rule (e.g. Russian): one / few / many.
    Russian,
}

/// A parsed gettext plural rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluralRule {
    nplurals: usize,
    kind: Kind,
}

impl Default for PluralRule {
    fn default() -> Self {
        PluralRule {
            nplurals: 2,
            kind: Kind::NotOne,
        }
    }
}

impl PluralRule {
    /// Parse a `Plural-Forms` header value (the part after `Plural-Forms:`).
    ///
    /// Accepts the full header line or just the value; tolerant of whitespace.
    /// Unknown plural expressions default to `(n != 1)` with `nplurals=2`.
    pub fn parse(plural_forms_header: &str) -> PluralRule {
        // Normalize: drop spaces so `n != 1` and `n!=1` both match.
        let normalized: String = plural_forms_header
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();

        let nplurals = parse_nplurals(&normalized).unwrap_or(2);

        // Match the Slavic 3-form rule by its distinctive `n%10==1&&n%100!=11`
        // signature, the single-form rule by `nplurals=1`/`plural=0`, then the
        // two 2-form rules; anything else defaults to the English `(n != 1)`.
        let kind = if normalized.contains("n%10==1&&n%100!=11") {
            Kind::Russian
        } else if nplurals == 1 || normalized.contains("plural=0;") || normalized.contains("plural=0")
        {
            Kind::Single
        } else if normalized.contains("plural=(n>1)") || normalized.contains("plural=n>1") {
            Kind::GreaterThanOne
        } else {
            // Default and explicit `(n != 1)` both land here.
            Kind::NotOne
        };

        PluralRule { nplurals, kind }
    }

    /// Number of plural forms (`nplurals`).
    pub fn nplurals(&self) -> usize {
        self.nplurals
    }

    /// Index of the plural form to use for count `n`.
    pub fn index(&self, n: i64) -> usize {
        match self.kind {
            Kind::NotOne => {
                if n == 1 {
                    0
                } else {
                    1
                }
            }
            Kind::GreaterThanOne => {
                if n > 1 {
                    1
                } else {
                    0
                }
            }
            // One form for every count (Japanese): always index 0.
            Kind::Single => 0,
            // Slavic one/few/many (Russian). Counts are non-negative here; guard
            // against negatives so the modulo arithmetic stays well-defined.
            Kind::Russian => {
                let n = n.unsigned_abs();
                if n % 10 == 1 && n % 100 != 11 {
                    0
                } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
                    1
                } else {
                    2
                }
            }
        }
    }
}

/// Extract the integer after `nplurals=` from a whitespace-stripped header.
fn parse_nplurals(normalized: &str) -> Option<usize> {
    let idx = normalized.find("nplurals=")? + "nplurals=".len();
    let rest = &normalized[idx..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nplurals_and_en_rule_index() {
        let rule = PluralRule::parse("nplurals=2; plural=(n != 1);");
        assert_eq!(rule.nplurals(), 2);
        // English / Spanish / German / Portuguese: 1 is singular.
        assert_eq!(rule.index(0), 1);
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(2), 1);
        assert_eq!(rule.index(5), 1);
    }

    #[test]
    fn parses_fr_greater_than_one_rule_index() {
        let rule = PluralRule::parse("nplurals=2; plural=(n > 1);");
        assert_eq!(rule.nplurals(), 2);
        // French: 0 and 1 are singular form.
        assert_eq!(rule.index(0), 0);
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(2), 1);
        assert_eq!(rule.index(10), 1);
    }

    #[test]
    fn parses_japanese_single_rule() {
        let rule = PluralRule::parse("nplurals=1; plural=0;");
        assert_eq!(rule.nplurals(), 1);
        // One form for every count.
        assert_eq!(rule.index(0), 0);
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(2), 0);
        assert_eq!(rule.index(100), 0);
    }

    #[test]
    fn parses_russian_three_form_rule() {
        let rule = PluralRule::parse(
            "nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<12 || n%100>14) ? 1 : 2);",
        );
        assert_eq!(rule.nplurals(), 3);
        // one: 1, 21, 31 (but not 11)
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(21), 0);
        assert_eq!(rule.index(11), 2);
        // few: 2-4, 22-24 (but not 12-14)
        assert_eq!(rule.index(2), 1);
        assert_eq!(rule.index(4), 1);
        assert_eq!(rule.index(23), 1);
        assert_eq!(rule.index(12), 2);
        // many: 0, 5-20, 25-30
        assert_eq!(rule.index(0), 2);
        assert_eq!(rule.index(5), 2);
        assert_eq!(rule.index(100), 2);
    }

    #[test]
    fn unknown_expression_falls_back_to_default() {
        let rule = PluralRule::parse("garbage header with no plural expr");
        assert_eq!(rule.nplurals(), 2);
        assert_eq!(rule.index(1), 0);
        assert_eq!(rule.index(3), 1);
    }
}
