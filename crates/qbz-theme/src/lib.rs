//! `qbz-theme` — the frontend-agnostic theme/palette registry (ADR-006).
//!
//! Pure Rust data + hand-rolled color/contrast math. NO Slint, NO Tauri, NO
//! heavy deps, so it compiles and unit-tests fast on its own and can be reused
//! by any frontend (Slint, the Tauri build, a TUI, contrast unit tests).
//!
//! The contract: a [`ThemeId`] maps to one fully-materialized [`ThemeColors`]
//! struct (no CSS cascade — every field is populated). The frontend converts
//! each [`Rgba`] to its own color type and pushes the struct into a single
//! theme global on theme change.
//!
//! Phase 1 materializes only the four existing themes; [`ThemeId::is_implemented`]
//! reports which rows are ready so the Settings list can expose only those.

pub mod auto;
mod color;
mod colors;
pub mod custom;
mod id;
mod registry;

pub use auto::{generate as generate_auto_theme, AutoSource};
pub use color::{apca_lc, contrast_ratio, relative_luminance, Rgba};
pub use colors::{alpha_byte, alpha_index, alpha_ramp, ThemeColors, ALPHA_COUNT, ALPHA_PERCENTS};
pub use custom::{base_from_theme, theme_from_base, CustomThemeBase};
pub use id::{default_slug, ThemeCategory, ThemeId, ALL};
pub use registry::palette;

/// A single entry in the Settings theme list: the stable id plus the data the
/// dropdown needs (display name, category, light/dark, ready flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeListEntry {
    pub id: ThemeId,
    pub display_name: &'static str,
    pub slug: &'static str,
    pub category: ThemeCategory,
    /// Luminance-derived (NOT the unreliable Tauri `type` flag): `true` when the
    /// theme's `surface_main` is light. Drives the dark/light list filter.
    pub is_light: bool,
    /// Whether the registry fully materializes this row yet (P1 gating).
    pub implemented: bool,
}

/// The default theme on a fresh profile (owner decision 2026-06-20: OLED Dark).
pub fn default_theme_id() -> ThemeId {
    ThemeId::default_id()
}

/// Whether a theme reads as "light" from its actual base surface luminance.
/// This is the corrected light/dark flag the plan mandates (Frost/Langley are
/// registered light in Tauri but are visually dark; Alucard is genuinely light).
pub fn is_light(id: ThemeId) -> bool {
    // System has no static palette; treat it as dark for filter purposes (it
    // follows the OS at runtime).
    if id == ThemeId::System {
        return false;
    }
    relative_luminance(palette(id).surface_main) >= 0.5
}

/// Whether a theme is one of the two High-Contrast accessibility themes.
/// Drives the Slint `Theme.is-high-contrast` flag, which gates HC-only
/// redundant-encoding affordances (1px control borders, slider-thumb borders)
/// so they never leak into the polished normal themes (P4 a11y pass).
pub fn is_high_contrast(id: ThemeId) -> bool {
    matches!(id, ThemeId::HighContrast | ThemeId::HighContrastLight)
}

/// Build the full Settings theme list in display order. The frontend filters by
/// `is_light` and may hide `!implemented` rows during P1/P2.
pub fn theme_list() -> Vec<ThemeListEntry> {
    ALL.iter()
        .map(|&id| ThemeListEntry {
            id,
            display_name: id.display_name(),
            slug: id.slug(),
            category: id.category(),
            is_light: is_light(id),
            implemented: id.is_implemented(),
        })
        .collect()
}

/// The implemented-only theme list (what the Settings dropdown shows in P1).
pub fn implemented_theme_list() -> Vec<ThemeListEntry> {
    theme_list().into_iter().filter(|e| e.implemented).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_oled() {
        assert_eq!(default_theme_id(), ThemeId::Oled);
    }

    #[test]
    fn registry_returns_populated_struct_for_default() {
        let c = palette(default_theme_id());
        // OLED: pure-black base, white text, full alpha ramp.
        assert_eq!(c.surface_main, Rgba::rgb(0, 0, 0));
        assert_eq!(c.text_primary, Rgba::rgb(0xff, 0xff, 0xff));
        assert_eq!(c.alpha.len(), ALPHA_COUNT);
    }

    #[test]
    fn light_dark_flag_from_luminance() {
        // The 4 P1 themes are all dark.
        assert!(!is_light(ThemeId::Dark));
        assert!(!is_light(ThemeId::Oled));
        assert!(!is_light(ThemeId::TokyoNight));
        assert!(!is_light(ThemeId::System));
    }

    #[test]
    fn implemented_list_is_every_theme() {
        // After P3 every theme is materialized — the standard rows AND the 5
        // redesigned accessibility themes (WcagLight/WcagDark/HighContrast/
        // HighContrastLight/Colorblind).
        let list = implemented_theme_list();
        assert_eq!(list.len(), ALL.len());
        let slugs: Vec<&str> = list.iter().map(|e| e.slug).collect();
        // P1 originals still present:
        assert!(slugs.contains(&"dark"));
        assert!(slugs.contains(&"oled"));
        assert!(slugs.contains(&"tokyo-night"));
        assert!(slugs.contains(&"system"));
        // P2 additions (spot-check across categories):
        assert!(slugs.contains(&"light"));
        assert!(slugs.contains(&"nord"));
        assert!(slugs.contains(&"dracula"));
        assert!(slugs.contains(&"frost"));
        assert!(slugs.contains(&"langley"));
        assert!(slugs.contains(&"alucard"));
        assert!(slugs.contains(&"kurosaki"));
        // P3 accessibility themes now implemented:
        assert!(slugs.contains(&"wcag-light"));
        assert!(slugs.contains(&"wcag-dark"));
        assert!(slugs.contains(&"high-contrast"));
        assert!(slugs.contains(&"high-contrast-light"));
        assert!(slugs.contains(&"colorblind"));
    }

    #[test]
    fn light_dark_filter_is_luminance_correct() {
        // Corrected flags: Alucard light; Frost/Langley dark despite Tauri type.
        assert!(is_light(ThemeId::Alucard));
        assert!(is_light(ThemeId::Light));
        assert!(is_light(ThemeId::SnowStorm));
        assert!(!is_light(ThemeId::Frost));
        assert!(!is_light(ThemeId::Langley));
        assert!(!is_light(ThemeId::Nord));
    }

    #[test]
    fn full_list_has_all_entries() {
        assert_eq!(theme_list().len(), ALL.len());
    }

    #[test]
    fn high_contrast_flag_only_for_hc_themes() {
        // True for exactly the two High-Contrast themes.
        assert!(is_high_contrast(ThemeId::HighContrast));
        assert!(is_high_contrast(ThemeId::HighContrastLight));
        // False for everything else, including the other a11y themes.
        assert!(!is_high_contrast(ThemeId::WcagLight));
        assert!(!is_high_contrast(ThemeId::WcagDark));
        assert!(!is_high_contrast(ThemeId::Colorblind));
        assert!(!is_high_contrast(ThemeId::Dark));
        assert!(!is_high_contrast(ThemeId::Oled));
        assert!(!is_high_contrast(ThemeId::System));
        // Exactly two themes in ALL are high-contrast.
        assert_eq!(ALL.iter().filter(|&&id| is_high_contrast(id)).count(), 2);
    }
}
