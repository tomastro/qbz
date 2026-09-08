//! Stable theme identity: the enum, its persisted slug, display name (proper-
//! noun data, NOT an i18n key — see ADR / i18n rule), category grouping, and a
//! luminance-derived light/dark flag.

use serde::{Deserialize, Serialize};

/// Grouping shown in the Settings theme list. Mirrors the Tauri SettingsView
/// registry comment blocks (Core / Dark / Light / Accessibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeCategory {
    Core,
    Dark,
    Light,
    Accessibility,
}

impl ThemeCategory {
    pub fn slug(self) -> &'static str {
        match self {
            ThemeCategory::Core => "core",
            ThemeCategory::Dark => "dark",
            ThemeCategory::Light => "light",
            ThemeCategory::Accessibility => "accessibility",
        }
    }
}

/// Every theme the registry can produce. The four marked "(P1)" are the only
/// ones materialized in Phase 1; the rest are placeholders the P2/P3 phases
/// fill in. `from_slug`/`slug` are stable across releases (persisted in
/// `ui_prefs.theme`), so the variant ORDER may change freely but the slugs must
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeId {
    // --- Core ---
    Dark,   // P1 — :root
    Oled,   // P1 — DEFAULT theme
    Light,
    System, // P1 — meta (OS-following; resolved in the frontend)
    // --- Dark (branded / community) ---
    Warm,
    Nord,
    Dracula,
    TokyoNight, // P1
    CatppuccinMocha,
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    BreezeDark,
    AdwaitaDark,
    Aurora,
    Ikari,
    Ayanami,
    Iscariot,
    Stratego,
    Rumi,
    Zoey,
    Mira,
    Frost,   // registered light, visually dark
    Langley, // registered light, visually dark
    // --- Light (branded / community) ---
    Alucard,
    RosePineDawn,
    BreezeLight,
    AdwaitaLight,
    DuotoneSnow,
    SnowStorm,
    Kurosaki,
    // --- Accessibility (REDESIGNED in P3) ---
    WcagLight,
    WcagDark,
    HighContrast,
    HighContrastLight,
    Colorblind,
}

/// All theme variants in display order (Core, Dark, Light, Accessibility).
pub const ALL: &[ThemeId] = &[
    ThemeId::Dark,
    ThemeId::Oled,
    ThemeId::Light,
    ThemeId::System,
    ThemeId::Warm,
    ThemeId::Nord,
    ThemeId::Dracula,
    ThemeId::TokyoNight,
    ThemeId::CatppuccinMocha,
    ThemeId::CatppuccinLatte,
    ThemeId::CatppuccinFrappe,
    ThemeId::CatppuccinMacchiato,
    ThemeId::BreezeDark,
    ThemeId::AdwaitaDark,
    ThemeId::Aurora,
    ThemeId::Ikari,
    ThemeId::Ayanami,
    ThemeId::Iscariot,
    ThemeId::Stratego,
    ThemeId::Rumi,
    ThemeId::Zoey,
    ThemeId::Mira,
    ThemeId::Frost,
    ThemeId::Langley,
    ThemeId::Alucard,
    ThemeId::RosePineDawn,
    ThemeId::BreezeLight,
    ThemeId::AdwaitaLight,
    ThemeId::DuotoneSnow,
    ThemeId::SnowStorm,
    ThemeId::Kurosaki,
    ThemeId::WcagLight,
    ThemeId::WcagDark,
    ThemeId::HighContrast,
    ThemeId::HighContrastLight,
    ThemeId::Colorblind,
];

impl ThemeId {
    /// The default theme on a fresh profile (owner decision 2026-06-20).
    pub const fn default_id() -> ThemeId {
        ThemeId::Oled
    }

    /// Whether this theme is fully materialized by the registry. Used by the
    /// frontend list builder to expose only ready themes during the phased
    /// rollout. After P3 every theme — including the 5 redesigned accessibility
    /// themes — is materialized, so this is now unconditionally `true`.
    pub fn is_implemented(self) -> bool {
        true
    }

    /// Stable persisted slug. MUST NOT change once shipped.
    pub fn slug(self) -> &'static str {
        match self {
            ThemeId::Dark => "dark",
            ThemeId::Oled => "oled",
            ThemeId::Light => "light",
            ThemeId::System => "system",
            ThemeId::Warm => "warm",
            ThemeId::Nord => "nord",
            ThemeId::Dracula => "dracula",
            ThemeId::TokyoNight => "tokyo-night",
            ThemeId::CatppuccinMocha => "catppuccin-mocha",
            ThemeId::CatppuccinLatte => "catppuccin-latte",
            ThemeId::CatppuccinFrappe => "catppuccin-frappe",
            ThemeId::CatppuccinMacchiato => "catppuccin-macchiato",
            ThemeId::BreezeDark => "breeze-dark",
            ThemeId::AdwaitaDark => "adwaita-dark",
            ThemeId::Aurora => "aurora",
            ThemeId::Ikari => "ikari",
            ThemeId::Ayanami => "ayanami",
            ThemeId::Iscariot => "iscariot",
            ThemeId::Stratego => "stratego",
            ThemeId::Rumi => "rumi",
            ThemeId::Zoey => "zoey",
            ThemeId::Mira => "mira",
            ThemeId::Frost => "frost",
            ThemeId::Langley => "langley",
            ThemeId::Alucard => "alucard",
            ThemeId::RosePineDawn => "rose-pine-dawn",
            ThemeId::BreezeLight => "breeze-light",
            ThemeId::AdwaitaLight => "adwaita-light",
            ThemeId::DuotoneSnow => "duotone-snow",
            ThemeId::SnowStorm => "snow-storm",
            ThemeId::Kurosaki => "kurosaki",
            ThemeId::WcagLight => "wcag-light",
            ThemeId::WcagDark => "wcag-dark",
            ThemeId::HighContrast => "high-contrast",
            ThemeId::HighContrastLight => "high-contrast-light",
            ThemeId::Colorblind => "colorblind",
        }
    }

    /// Parse a persisted slug back to a `ThemeId`. Unknown slugs return `None`
    /// (the caller falls back to the default).
    pub fn from_slug(s: &str) -> Option<ThemeId> {
        ALL.iter().copied().find(|id| id.slug() == s)
    }

    /// Human-facing display name. This is proper-noun DATA (theme names like
    /// "Nord", "Tokyo Night", "OLED Black") — NOT a translatable UI string, so
    /// it lives here in the registry, not in the i18n catalog.
    pub fn display_name(self) -> &'static str {
        match self {
            ThemeId::Dark => "Dark",
            ThemeId::Oled => "OLED Black",
            ThemeId::Light => "Light",
            ThemeId::System => "System",
            ThemeId::Warm => "Warm",
            ThemeId::Nord => "Nord",
            ThemeId::Dracula => "Dracula",
            ThemeId::TokyoNight => "Tokyo Night",
            ThemeId::CatppuccinMocha => "Catppuccin Mocha",
            ThemeId::CatppuccinLatte => "Catppuccin Latte",
            ThemeId::CatppuccinFrappe => "Catppuccin Frappé",
            ThemeId::CatppuccinMacchiato => "Catppuccin Macchiato",
            ThemeId::BreezeDark => "Breeze Dark",
            ThemeId::AdwaitaDark => "Adwaita Dark",
            ThemeId::Aurora => "Aurora",
            ThemeId::Ikari => "Ikari",
            ThemeId::Ayanami => "Ayanami",
            ThemeId::Iscariot => "Iscariot",
            ThemeId::Stratego => "Stratego",
            ThemeId::Rumi => "Rumi",
            ThemeId::Zoey => "Zoey",
            ThemeId::Mira => "Mira",
            ThemeId::Frost => "Frost",
            ThemeId::Langley => "Langley",
            ThemeId::Alucard => "Alucard",
            ThemeId::RosePineDawn => "Rose Pine Dawn",
            ThemeId::BreezeLight => "Breeze Light",
            ThemeId::AdwaitaLight => "Adwaita Light",
            ThemeId::DuotoneSnow => "Duotone Snow",
            ThemeId::SnowStorm => "Snow Storm",
            ThemeId::Kurosaki => "Kurosaki",
            ThemeId::WcagLight => "WCAG Light",
            ThemeId::WcagDark => "WCAG Dark",
            ThemeId::HighContrast => "High Contrast",
            ThemeId::HighContrastLight => "High Contrast Light",
            ThemeId::Colorblind => "Colorblind",
        }
    }

    /// Category grouping for the Settings list. Note the corrected placement of
    /// `Frost`/`Langley` (visually dark despite the Tauri `type:light` flag) and
    /// `Alucard` (a genuine light theme grouped under "Dark" in Tauri).
    pub fn category(self) -> ThemeCategory {
        use ThemeId::*;
        match self {
            Dark | Oled | Light | System => ThemeCategory::Core,
            Warm | Nord | Dracula | TokyoNight | CatppuccinMocha | CatppuccinFrappe
            | CatppuccinMacchiato | BreezeDark | AdwaitaDark
            | Aurora | Ikari | Ayanami | Iscariot | Stratego | Rumi | Zoey | Mira | Frost
            | Langley => ThemeCategory::Dark,
            // Catppuccin Latte is the LIGHT flavor — groups with the light themes.
            Alucard | CatppuccinLatte | RosePineDawn | BreezeLight | AdwaitaLight | DuotoneSnow
            | SnowStorm | Kurosaki => ThemeCategory::Light,
            WcagLight | WcagDark | HighContrast | HighContrastLight | Colorblind => {
                ThemeCategory::Accessibility
            }
        }
    }
}

/// The default theme slug (`"oled"`). Convenience for `ui_prefs::default_theme`.
pub fn default_slug() -> &'static str {
    ThemeId::default_id().slug()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_roundtrip_all() {
        for &id in ALL {
            assert_eq!(ThemeId::from_slug(id.slug()), Some(id), "slug {} failed", id.slug());
        }
    }

    #[test]
    fn slugs_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for &id in ALL {
            assert!(seen.insert(id.slug()), "duplicate slug {}", id.slug());
        }
        assert_eq!(seen.len(), ALL.len());
    }

    #[test]
    fn default_is_oled() {
        assert_eq!(ThemeId::default_id(), ThemeId::Oled);
        assert_eq!(default_slug(), "oled");
    }

    #[test]
    fn unknown_slug_is_none() {
        assert_eq!(ThemeId::from_slug("does-not-exist"), None);
    }

    #[test]
    fn p1_themes_implemented() {
        for id in [ThemeId::Dark, ThemeId::Oled, ThemeId::TokyoNight, ThemeId::System] {
            assert!(id.is_implemented(), "{:?} should be P1-implemented", id);
        }
    }
}
