//! The fully-materialized per-theme color set.
//!
//! Every theme row in the registry returns one of these with EVERY field
//! populated — there is no CSS cascade on the Slint side, so omissions in
//! `src/app.css` are resolved against `:root` (Dark) at registry-build time.

use crate::color::Rgba;

/// The 24 alpha tiers, in ascending percentage order. This is the SUPERSET of
/// the two Tauri alpha scales (cosmetic 20-tier + a11y 22-tier), per the
/// migration plan (A.3). Index into [`ThemeColors::alpha`] with [`AlphaTier`].
pub const ALPHA_PERCENTS: [u8; 24] = [
    4, 5, 6, 8, 10, 12, 15, 18, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95,
];

/// Number of alpha tiers (= `ALPHA_PERCENTS.len()`).
pub const ALPHA_COUNT: usize = 24;

/// Map an alpha percentage to its `0xAA` byte (rounded). `pct * 255 / 100`.
pub const fn alpha_byte(pct: u8) -> u8 {
    ((pct as u16 * 255 + 50) / 100) as u8
}

/// Position of an alpha percentage within [`ALPHA_PERCENTS`], or `None`.
pub fn alpha_index(pct: u8) -> Option<usize> {
    let mut i = 0;
    while i < ALPHA_COUNT {
        if ALPHA_PERCENTS[i] == pct {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The complete, frontend-agnostic color contract for one theme. Field order
/// groups by family (surfaces, text, accent, danger, warning, success, borders,
/// focus, extras, alpha) to match the Slint `ThemeColors` struct and the plan's
/// A.3 token list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeColors {
    // --- surfaces ---
    pub surface_main: Rgba,
    pub surface_card: Rgba,
    pub surface_elevated: Rgba,
    /// Alpha-based hover overlay (translucent, polarity-correct).
    pub surface_hover: Rgba,
    /// Opaque theme `--bg-hover` hex (distinct from the alpha `surface_hover`).
    pub bg_hover: Rgba,

    // --- text ---
    pub text_primary: Rgba,
    pub text_secondary: Rgba,
    pub text_muted: Rgba,
    pub text_disabled: Rgba,

    // --- accent ---
    pub accent: Rgba,
    pub accent_hover: Rgba,
    pub accent_pressed: Rgba,
    /// Text drawn ON an accent fill (`--btn-primary-text`).
    pub accent_text: Rgba,

    // --- danger family ---
    pub danger: Rgba,
    pub danger_bg: Rgba,
    pub danger_border: Rgba,
    pub danger_hover: Rgba,

    // --- warning family ---
    pub warning: Rgba,
    pub warning_bg: Rgba,
    pub warning_border: Rgba,
    pub warning_hover: Rgba,

    // --- success family (NEW vs Tauri parity) ---
    pub success: Rgba,
    pub success_bg: Rgba,
    pub success_border: Rgba,
    pub success_hover: Rgba,

    // --- borders ---
    /// Theme `--border-subtle` value. The legacy Slint `border-subtle` alias was
    /// a translucent white hairline; for the P1 themes this keeps that value so
    /// the 4 themes stay pixel-identical. Standard/a11y rows (P2/P3) feed the
    /// theme `--border-subtle` hex.
    pub border_subtle: Rgba,
    /// Legacy Slint-only token (no Tauri equivalent): a stronger translucent
    /// edge used by popovers/dropdowns. Kept so existing call sites compile.
    pub border_muted: Rgba,
    pub border_strong: Rgba,

    // --- focus (NEW) ---
    pub focus_ring: Rgba,

    // --- extras ---
    pub favorite: Rgba,
    pub card_shadow: Rgba,

    // --- alpha overlays (polarity baked in: white on dark, black on light) ---
    pub alpha: [Rgba; ALPHA_COUNT],
}

impl ThemeColors {
    /// Look up an alpha overlay by percentage (e.g. `8`, `55`). Falls back to
    /// the nearest standard tier if an exact match is absent.
    pub fn alpha_pct(&self, pct: u8) -> Rgba {
        if let Some(i) = alpha_index(pct) {
            return self.alpha[i];
        }
        // Nearest tier by absolute distance.
        let mut best = 0usize;
        let mut best_d = u8::MAX as i32;
        for (i, &p) in ALPHA_PERCENTS.iter().enumerate() {
            let d = (p as i32 - pct as i32).abs();
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        self.alpha[best]
    }
}

/// Build the 24-tier alpha ramp for a theme of the given polarity.
/// `is_light` themes get a BLACK base (dark hairlines/hovers read on light
/// surfaces); dark themes get a WHITE base — matching Tauri's per-theme flip.
pub fn alpha_ramp(is_light: bool) -> [Rgba; ALPHA_COUNT] {
    let (r, g, b) = if is_light { (0, 0, 0) } else { (255, 255, 255) };
    let mut out = [Rgba::rgba(r, g, b, 0); ALPHA_COUNT];
    let mut i = 0;
    while i < ALPHA_COUNT {
        out[i] = Rgba::rgba(r, g, b, alpha_byte(ALPHA_PERCENTS[i]));
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_byte_rounds() {
        assert_eq!(alpha_byte(8), 0x14); // 8% -> 20
        assert_eq!(alpha_byte(10), 0x1a); // 10% -> 26
        assert_eq!(alpha_byte(12), 0x1f); // 12% -> 31
        assert_eq!(alpha_byte(18), 0x2e); // 18% -> 46
        assert_eq!(alpha_byte(55), 0x8c); // 55% -> 140
        assert_eq!(alpha_byte(65), 0xa6); // 65% -> 166
        assert_eq!(alpha_byte(70), 0xb3); // 70% -> 179
        assert_eq!(alpha_byte(75), 0xbf); // 75% -> 191
    }

    #[test]
    fn ramp_polarity() {
        let dark = alpha_ramp(false);
        let light = alpha_ramp(true);
        assert_eq!(dark[alpha_index(8).unwrap()], Rgba::rgba(255, 255, 255, 0x14));
        assert_eq!(light[alpha_index(8).unwrap()], Rgba::rgba(0, 0, 0, 0x14));
    }

    #[test]
    fn alpha_count_is_24() {
        assert_eq!(ALPHA_COUNT, 24);
        assert_eq!(ALPHA_PERCENTS.len(), 24);
    }
}
