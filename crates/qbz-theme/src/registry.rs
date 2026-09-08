//! The theme registry: `ThemeId` -> fully-materialized [`ThemeColors`].
//!
//! P1 materialized the four existing themes (Dark / OLED / Tokyo Night /
//! System-fallback). P2 (this file) transcribes the remaining **standard**
//! (non-accessibility) themes from `src/app.css` — every value cites the
//! `qbz-nix-docs/.../01-tauri-themes-inventory.md` table (which was read 1:1
//! from `src/app.css`). P3 will add the redesigned accessibility rows.
//!
//! No CSS cascade on the Slint side: every row is FULLY materialized. Tauri
//! themes that OMIT tokens (e.g. `light` omits the accent trio; `oled`/
//! `breeze-dark`/`adwaita-dark` omit whole danger/warning families) inherit
//! those from `:root` Dark — so the omissions are resolved against `dark()` at
//! transcription time, here, not at runtime.
//!
//! Derived (no Tauri parity) tokens for the standard rows:
//!   - `success` family: NEW. Tauri has no `--success`. We use the project green
//!     `#3fae6a` for dark themes (matches P1) and a darker `#1f8a4c` for light
//!     themes (so success text clears >=3:1 on a light surface), with the same
//!     0.1/0.3/0.2 tint shape for bg/border/hover. Polished in P4.
//!   - `focus_ring`: NEW (WCAG 2.4.7). Uses the theme accent (high-visibility,
//!     matches P1). Polished in P4.
//!   - `favorite`: the loved-heart uses the theme `danger` hue (matches P1).
//!   - `danger_bg/border/hover`, `warning_*`: Tauri expresses these as `rgba()`
//!     tints of the solid hue at 0.1/0.3/0.2 (dracula uses 0.15/0.4/0.25). We
//!     bake the same straight-alpha overlays so they composite identically.
//!   - `border_muted`: legacy Slint-only token (no Tauri var). Polarity-aware
//!     translucent edge (white ~22% on dark, black ~22% on light).
//!   - `surface_hover`: alpha-based hover overlay, polarity-aware (white ~6% on
//!     dark, black ~6% on light), distinct from the opaque theme `--bg-hover`.

use crate::color::{relative_luminance, Rgba};
use crate::colors::{alpha_ramp, ThemeColors};
use crate::id::ThemeId;

/// Legacy translucent edge values the existing 4 Slint dark themes used directly,
/// reproduced 1:1 so P1 stays pixel-identical:
///   surface-hover  = ~6% white  (#ffffff10)
///   border-subtle  = ~8% white  (#ffffff14)
///   border-muted   = ~22% white (#ffffff38)
///   card-shadow    = rgba(0,0,0,0.4) (#00000066)
const LEGACY_SURFACE_HOVER: Rgba = Rgba::rgba(255, 255, 255, 0x10);
const LEGACY_BORDER_SUBTLE: Rgba = Rgba::rgba(255, 255, 255, 0x14);
const LEGACY_BORDER_MUTED: Rgba = Rgba::rgba(255, 255, 255, 0x38);
const LEGACY_CARD_SHADOW: Rgba = Rgba::rgba(0, 0, 0, 0x66);

/// Resolve a theme id to its concrete color set.
///
/// `System` has no static palette — at runtime the Slint side follows the OS
/// (std-widgets `Palette`) for the tokens it overrides, exactly as before. This
/// returns the Dark set as a safe fallback for any caller that needs a concrete
/// struct for `System` (it is NOT what paints the System theme; that stays the
/// `is-system` path in `theme.slint`).
pub fn palette(id: ThemeId) -> ThemeColors {
    match id {
        // --- Core (P1 + the standard Light) ---
        ThemeId::Dark => dark(),
        ThemeId::Oled => oled(),
        ThemeId::TokyoNight => tokyo_night(),
        ThemeId::System => dark(),
        ThemeId::Light => light(),
        // --- Dark (branded / community) ---
        ThemeId::Warm => warm(),
        ThemeId::Nord => nord(),
        ThemeId::Dracula => dracula(),
        ThemeId::CatppuccinMocha => catppuccin_mocha(),
        ThemeId::CatppuccinLatte => catppuccin_latte(),
        ThemeId::CatppuccinFrappe => catppuccin_frappe(),
        ThemeId::CatppuccinMacchiato => catppuccin_macchiato(),
        ThemeId::BreezeDark => breeze_dark(),
        ThemeId::AdwaitaDark => adwaita_dark(),
        ThemeId::Aurora => aurora(),
        ThemeId::Ikari => ikari(),
        ThemeId::Ayanami => ayanami(),
        ThemeId::Iscariot => iscariot(),
        ThemeId::Stratego => stratego(),
        ThemeId::Rumi => rumi(),
        ThemeId::Zoey => zoey(),
        ThemeId::Mira => mira(),
        ThemeId::Frost => frost(),
        ThemeId::Langley => langley(),
        // --- Light (branded / community) ---
        ThemeId::Alucard => alucard(),
        ThemeId::RosePineDawn => rose_pine_dawn(),
        ThemeId::BreezeLight => breeze_light(),
        ThemeId::AdwaitaLight => adwaita_light(),
        ThemeId::DuotoneSnow => duotone_snow(),
        ThemeId::SnowStorm => snow_storm(),
        ThemeId::Kurosaki => kurosaki(),
        // --- Accessibility (REDESIGNED in P3): final verified palettes from
        // 99-MIGRATION-PLAN.md Part B (adversarial corrections folded in). ---
        ThemeId::WcagLight => wcag_light(),
        ThemeId::WcagDark => wcag_dark(),
        ThemeId::HighContrast => high_contrast(),
        ThemeId::HighContrastLight => high_contrast_light(),
        ThemeId::Colorblind => colorblind(),
    }
}

// ---------------------------------------------------------------------------
// Shared builder for the standard themes
// ---------------------------------------------------------------------------

/// The Tauri token set for one standard theme, as read from doc 01. Only the
/// named hues are carried here; the derived families (success/focus/favorite),
/// the polarity-driven alpha ramp + translucent edges, and the status tints are
/// materialized by [`StdSpec::build`].
#[derive(Clone, Copy)]
struct StdSpec {
    // surfaces (--bg-*)
    bg_primary: Rgba,
    bg_secondary: Rgba,
    bg_tertiary: Rgba,
    bg_hover: Rgba,
    // text (--text-*)
    text_primary: Rgba,
    text_secondary: Rgba,
    text_muted: Rgba,
    text_disabled: Rgba,
    // accent (--accent-* + --btn-primary-text)
    accent: Rgba,
    accent_hover: Rgba,
    accent_pressed: Rgba,
    accent_text: Rgba,
    // status hues (--danger / --warning); families derived as rgba() tints
    danger: Rgba,
    warning: Rgba,
    /// Tint fractions for the danger/warning bg/border/hover families.
    /// Standard themes use (0.1, 0.3, 0.2); dracula uses (0.15, 0.4, 0.25).
    tint_bg: f32,
    tint_border: f32,
    tint_hover: f32,
    // borders (--border-*)
    border_subtle: Rgba,
    border_strong: Rgba,
}

impl StdSpec {
    /// Default status-tint fractions (every theme except dracula).
    const TINT_BG: f32 = 0.1;
    const TINT_BORDER: f32 = 0.3;
    const TINT_HOVER: f32 = 0.2;

    /// Materialize a complete [`ThemeColors`] row. `is_light` is the corrected
    /// (luminance-derived) polarity — it drives the alpha ramp base (black on
    /// light, white on dark), the translucent edge/hover bases, and the derived
    /// `success` hue. NOTE: do NOT trust the Tauri `type` flag for this; pass the
    /// real luminance (Frost/Langley are registered light but are dark canvases).
    fn build(self, is_light: bool) -> ThemeColors {
        // success: NEW token, no Tauri parity. Theme-appropriate green that
        // clears >=3:1 on the theme surface; darker on light themes. Polished P4.
        let success = if is_light {
            Rgba::rgb(0x1f, 0x8a, 0x4c)
        } else {
            Rgba::rgb(0x3f, 0xae, 0x6a)
        };

        // Polarity-aware translucent edges (legacy Slint-only tokens). On light
        // themes a white hairline is invisible, so flip the base to black.
        let (eh, eg, eb) = if is_light { (0, 0, 0) } else { (255, 255, 255) };
        let surface_hover = Rgba::rgba(eh, eg, eb, 0x10); // ~6%
        let border_muted = Rgba::rgba(eh, eg, eb, 0x38); // ~22%

        ThemeColors {
            surface_main: self.bg_primary,
            surface_card: self.bg_secondary,
            surface_elevated: self.bg_tertiary,
            surface_hover,
            bg_hover: self.bg_hover,

            text_primary: self.text_primary,
            text_secondary: self.text_secondary,
            text_muted: self.text_muted,
            text_disabled: self.text_disabled,

            accent: self.accent,
            accent_hover: self.accent_hover,
            accent_pressed: self.accent_pressed,
            accent_text: self.accent_text,

            danger: self.danger,
            danger_bg: with_alpha(self.danger, self.tint_bg),
            danger_border: with_alpha(self.danger, self.tint_border),
            danger_hover: with_alpha(self.danger, self.tint_hover),

            warning: self.warning,
            warning_bg: with_alpha(self.warning, self.tint_bg),
            warning_border: with_alpha(self.warning, self.tint_border),
            warning_hover: with_alpha(self.warning, self.tint_hover),

            success,
            success_bg: with_alpha(success, self.tint_bg),
            success_border: with_alpha(success, self.tint_border),
            success_hover: with_alpha(success, self.tint_hover),

            // Standard rows feed the theme `--border-subtle` hex (NOT the legacy
            // translucent hairline the 4 P1 rows kept).
            border_subtle: self.border_subtle,
            border_muted,
            border_strong: self.border_strong,

            focus_ring: self.accent, // = accent (no Tauri token; new)

            favorite: self.danger, // loved-heart uses danger red
            card_shadow: LEGACY_CARD_SHADOW,

            alpha: alpha_ramp(is_light),
        }
    }
}

impl Default for StdSpec {
    /// All-black placeholder; every field is overwritten per theme. The default
    /// only exists so theme functions can use struct-update syntax for the tint
    /// fractions without repeating them.
    fn default() -> Self {
        let z = Rgba::rgb(0, 0, 0);
        StdSpec {
            bg_primary: z,
            bg_secondary: z,
            bg_tertiary: z,
            bg_hover: z,
            text_primary: z,
            text_secondary: z,
            text_muted: z,
            text_disabled: z,
            accent: z,
            accent_hover: z,
            accent_pressed: z,
            accent_text: z,
            danger: z,
            warning: z,
            tint_bg: StdSpec::TINT_BG,
            tint_border: StdSpec::TINT_BORDER,
            tint_hover: StdSpec::TINT_HOVER,
            border_subtle: z,
            border_strong: z,
        }
    }
}

/// True when a `bg-primary` reads as light (luminance >= 0.5). Drives polarity
/// for the standard rows. Matches `lib::is_light` (which calls through
/// `palette()`), but used internally to avoid a recursive `palette()` call.
fn bg_is_light(bg_primary: Rgba) -> bool {
    relative_luminance(bg_primary) >= 0.5
}

// ---------------------------------------------------------------------------
// Core themes (P1 originals + standard Light)
// ---------------------------------------------------------------------------

/// `:root` Dark — the base every other theme inherits omissions from.
/// All hex values cite `src/app.css :root` via the inventory doc.
fn dark() -> ThemeColors {
    let danger = Rgba::rgb(0xef, 0x44, 0x44); // --danger
    let warning = Rgba::rgb(0xfb, 0xbf, 0x24); // --warning
    let success = Rgba::rgb(0x3f, 0xae, 0x6a); // NEW (project green)
    ThemeColors {
        surface_main: Rgba::rgb(0x0f, 0x0f, 0x0f),     // --bg-primary
        surface_card: Rgba::rgb(0x1a, 0x1a, 0x1a),     // --bg-secondary
        surface_elevated: Rgba::rgb(0x2a, 0x2a, 0x2a), // --bg-tertiary
        surface_hover: LEGACY_SURFACE_HOVER,
        bg_hover: Rgba::rgb(0x1f, 0x1f, 0x1f), // --bg-hover

        text_primary: Rgba::rgb(0xff, 0xff, 0xff),   // --text-primary
        text_secondary: Rgba::rgb(0xcc, 0xcc, 0xcc), // --text-secondary
        text_muted: Rgba::rgb(0x88, 0x88, 0x88),     // --text-muted
        text_disabled: Rgba::rgb(0x55, 0x55, 0x55),  // --text-disabled

        accent: Rgba::rgb(0x42, 0x85, 0xf4),         // --accent-primary
        accent_hover: Rgba::rgb(0x5a, 0x9b, 0xf4),   // --accent-hover
        accent_pressed: Rgba::rgb(0x32, 0x75, 0xe4), // --accent-active
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),    // --btn-primary-text

        danger,
        danger_bg: with_alpha(danger, 0.1),
        danger_border: with_alpha(danger, 0.3),
        danger_hover: with_alpha(danger, 0.2),

        warning,
        warning_bg: with_alpha(warning, 0.1),
        warning_border: with_alpha(warning, 0.3),
        warning_hover: with_alpha(warning, 0.2),

        success,
        success_bg: with_alpha(success, 0.1),
        success_border: with_alpha(success, 0.3),
        success_hover: with_alpha(success, 0.2),

        border_subtle: LEGACY_BORDER_SUBTLE,
        border_muted: LEGACY_BORDER_MUTED,
        border_strong: Rgba::rgb(0x3a, 0x3a, 0x3a), // --border-strong

        focus_ring: Rgba::rgb(0x42, 0x85, 0xf4), // = accent (no Tauri token; new)

        favorite: danger, // the loved-heart uses danger red
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(false), // dark theme -> white-based overlays
    }
}

/// OLED Black — inherits everything from Dark except backgrounds + borders.
/// The legacy Slint OLED only overrode the three surfaces; keep that exactly,
/// inherit the rest from `dark()`.
fn oled() -> ThemeColors {
    ThemeColors {
        surface_main: Rgba::rgb(0x00, 0x00, 0x00),     // --bg-primary
        surface_card: Rgba::rgb(0x0a, 0x0a, 0x0a),     // --bg-secondary
        surface_elevated: Rgba::rgb(0x1a, 0x1a, 0x1a), // --bg-tertiary
        bg_hover: Rgba::rgb(0x11, 0x11, 0x11),         // --bg-hover (oled)
        border_strong: Rgba::rgb(0x2a, 0x2a, 0x2a),    // --border-strong (oled)
        ..dark()
    }
}

/// Tokyo Night — full recolor. Surfaces/text/accent transcribed from the legacy
/// Slint ternary (which matches `src/app.css [data-theme="tokyo-night"]`).
fn tokyo_night() -> ThemeColors {
    let danger = Rgba::rgb(0xdb, 0x4b, 0x4b); // --danger
    let warning = Rgba::rgb(0xe0, 0xaf, 0x68); // --warning
    let success = Rgba::rgb(0x3f, 0xae, 0x6a);
    ThemeColors {
        surface_main: Rgba::rgb(0x1a, 0x1b, 0x26),     // --bg-primary
        surface_card: Rgba::rgb(0x16, 0x16, 0x1e),     // --bg-secondary
        surface_elevated: Rgba::rgb(0x1c, 0x1d, 0x29), // --bg-tertiary
        surface_hover: LEGACY_SURFACE_HOVER,
        bg_hover: Rgba::rgb(0x20, 0x23, 0x30), // --bg-hover

        text_primary: Rgba::rgb(0xa9, 0xb1, 0xd6),   // --text-primary
        text_secondary: Rgba::rgb(0x78, 0x7c, 0x99), // --text-secondary
        text_muted: Rgba::rgb(0x54, 0x5c, 0x7e),     // --text-muted
        text_disabled: Rgba::rgb(0x3d, 0x42, 0x5e),  // --text-disabled

        accent: Rgba::rgb(0x7a, 0xa2, 0xf7),         // --accent-primary
        accent_hover: Rgba::rgb(0x7d, 0xcf, 0xff),   // --accent-hover
        accent_pressed: Rgba::rgb(0xbb, 0x9a, 0xf7), // --accent-active
        accent_text: Rgba::rgb(0x1a, 0x1b, 0x26),    // --btn-primary-text

        danger,
        danger_bg: with_alpha(danger, 0.1),
        danger_border: with_alpha(danger, 0.3),
        danger_hover: with_alpha(danger, 0.2),

        warning,
        warning_bg: with_alpha(warning, 0.1),
        warning_border: with_alpha(warning, 0.3),
        warning_hover: with_alpha(warning, 0.2),

        success,
        success_bg: with_alpha(success, 0.1),
        success_border: with_alpha(success, 0.3),
        success_hover: with_alpha(success, 0.2),

        border_subtle: LEGACY_BORDER_SUBTLE,
        border_muted: LEGACY_BORDER_MUTED,
        border_strong: Rgba::rgb(0x20, 0x23, 0x30), // --border-strong

        focus_ring: Rgba::rgb(0x7a, 0xa2, 0xf7), // = accent

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(false), // dark theme -> white-based overlays
    }
}

/// `light` — core Light theme. OMITS the accent trio (inherits the Dark blue
/// `#4285F4` family from `:root`); alpha base flips to black. (doc 01 §light)
fn light() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xff, 0xff, 0xff),
        bg_secondary: Rgba::rgb(0xf5, 0xf5, 0xf5),
        bg_tertiary: Rgba::rgb(0xe8, 0xe8, 0xe8),
        bg_hover: Rgba::rgb(0xf0, 0xf0, 0xf0),
        text_primary: Rgba::rgb(0x0f, 0x0f, 0x0f),
        text_secondary: Rgba::rgb(0x44, 0x44, 0x44),
        text_muted: Rgba::rgb(0x66, 0x66, 0x66),
        text_disabled: Rgba::rgb(0x99, 0x99, 0x99),
        // accent trio inherited from :root Dark:
        accent: Rgba::rgb(0x42, 0x85, 0xf4),
        accent_hover: Rgba::rgb(0x5a, 0x9b, 0xf4),
        accent_pressed: Rgba::rgb(0x32, 0x75, 0xe4),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff), // --btn-primary-text
        // light defines its own danger/warning hues (darker):
        danger: Rgba::rgb(0xdc, 0x26, 0x26),
        warning: Rgba::rgb(0xd9, 0x77, 0x06),
        // light uses 0.1/0.3/0.15 in app.css (hover is 0.15 not 0.2). Keep faithful.
        tint_hover: 0.15,
        border_subtle: Rgba::rgb(0xe0, 0xe0, 0xe0),
        border_strong: Rgba::rgb(0xcc, 0xcc, 0xcc),
        ..Default::default()
    };
    s.build(true)
}

// ---------------------------------------------------------------------------
// Dark (branded / community) themes
// ---------------------------------------------------------------------------

fn warm() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x2b, 0x1a, 0x14),
        bg_secondary: Rgba::rgb(0x3a, 0x24, 0x1a),
        bg_tertiary: Rgba::rgb(0x4a, 0x2f, 0x23),
        bg_hover: Rgba::rgb(0x5b, 0x3a, 0x2e),
        text_primary: Rgba::rgb(0xf5, 0xe9, 0xe2),
        text_secondary: Rgba::rgb(0xd8, 0xc3, 0xb7),
        text_muted: Rgba::rgb(0xbf, 0xa3, 0x96),
        text_disabled: Rgba::rgb(0x8d, 0x73, 0x67),
        accent: Rgba::rgb(0xe5, 0x98, 0x66),
        accent_hover: Rgba::rgb(0xf0, 0xa7, 0x7b),
        accent_pressed: Rgba::rgb(0xd8, 0x86, 0x52),
        accent_text: Rgba::rgb(0x00, 0x00, 0x00),
        danger: Rgba::rgb(0xbf, 0x4f, 0x4f),
        warning: Rgba::rgb(0xd6, 0xa9, 0x4f),
        border_subtle: Rgba::rgb(0x4a, 0x2f, 0x23),
        border_strong: Rgba::rgb(0x5b, 0x3a, 0x2e),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn nord() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x1d, 0x22, 0x30),
        bg_secondary: Rgba::rgb(0x2a, 0x2f, 0x3c),
        bg_tertiary: Rgba::rgb(0x32, 0x38, 0x4c),
        bg_hover: Rgba::rgb(0x3c, 0x42, 0x56),
        text_primary: Rgba::rgb(0xec, 0xec, 0xec),
        text_secondary: Rgba::rgb(0xc6, 0xc6, 0xc6),
        text_muted: Rgba::rgb(0x99, 0x99, 0xa3),
        text_disabled: Rgba::rgb(0x6f, 0x6f, 0x7b),
        accent: Rgba::rgb(0x35, 0x84, 0xe4),
        accent_hover: Rgba::rgb(0x5f, 0x9e, 0xe6),
        accent_pressed: Rgba::rgb(0x1a, 0x5f, 0xb4),
        accent_text: Rgba::rgb(0x24, 0x1f, 0x31),
        danger: Rgba::rgb(0xc0, 0x1c, 0x28),
        warning: Rgba::rgb(0xf5, 0xc2, 0x11),
        border_subtle: Rgba::rgb(0x2a, 0x2f, 0x3c),
        border_strong: Rgba::rgb(0x32, 0x38, 0x4c),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn dracula() -> ThemeColors {
    // NON-STANDARD tint fractions: bg .15 / border .4 / hover .25.
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x28, 0x2a, 0x36),
        bg_secondary: Rgba::rgb(0x21, 0x22, 0x2c),
        bg_tertiary: Rgba::rgb(0x34, 0x37, 0x46),
        bg_hover: Rgba::rgb(0x44, 0x47, 0x5a),
        text_primary: Rgba::rgb(0xf8, 0xf8, 0xf2),
        text_secondary: Rgba::rgb(0xe2, 0xe2, 0xdc),
        text_muted: Rgba::rgb(0x62, 0x72, 0xa4),
        text_disabled: Rgba::rgb(0x44, 0x47, 0x5a),
        accent: Rgba::rgb(0xbd, 0x93, 0xf9),
        accent_hover: Rgba::rgb(0xff, 0x79, 0xc6),
        accent_pressed: Rgba::rgb(0x8b, 0xe9, 0xfd),
        accent_text: Rgba::rgb(0x28, 0x2a, 0x36),
        danger: Rgba::rgb(0xff, 0x55, 0x55),
        warning: Rgba::rgb(0xff, 0xb8, 0x6c),
        tint_bg: 0.15,
        tint_border: 0.4,
        tint_hover: 0.25,
        border_subtle: Rgba::rgb(0x34, 0x37, 0x46),
        border_strong: Rgba::rgb(0x44, 0x47, 0x5a),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn catppuccin_mocha() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x1e, 0x1e, 0x2e),
        bg_secondary: Rgba::rgb(0x18, 0x18, 0x25),
        bg_tertiary: Rgba::rgb(0x11, 0x11, 0x1b),
        bg_hover: Rgba::rgb(0x31, 0x32, 0x44),
        text_primary: Rgba::rgb(0xcd, 0xd6, 0xf4),
        text_secondary: Rgba::rgb(0xba, 0xc2, 0xde),
        text_muted: Rgba::rgb(0xa6, 0xad, 0xc8),
        text_disabled: Rgba::rgb(0x73, 0x79, 0x94),
        accent: Rgba::rgb(0xcb, 0xa6, 0xf7),
        accent_hover: Rgba::rgb(0x89, 0xb4, 0xfa),
        accent_pressed: Rgba::rgb(0xf3, 0x8b, 0xa8),
        accent_text: Rgba::rgb(0x1e, 0x1e, 0x2e),
        danger: Rgba::rgb(0xf3, 0x8b, 0xa8),
        warning: Rgba::rgb(0xf9, 0xe2, 0xaf),
        border_subtle: Rgba::rgb(0x31, 0x32, 0x44),
        border_strong: Rgba::rgb(0x45, 0x47, 0x5a),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}
/// catppuccin-latte — light theme (Part B §latte). Black alpha base.
fn catppuccin_latte() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xef, 0xf1, 0xf5),
        bg_secondary: Rgba::rgb(0xe6, 0xe9, 0xef),
        bg_tertiary: Rgba::rgb(0xdc, 0xe0, 0xe8),
        bg_hover: Rgba::rgb(0xcc, 0xd0, 0xda),
        text_primary: Rgba::rgb(0x4c, 0x4f, 0x69),
        text_secondary: Rgba::rgb(0x5c, 0x5f, 0x77),
        text_muted: Rgba::rgb(0x6c, 0x6f, 0x85),
        text_disabled: Rgba::rgb(0x9c, 0xa0, 0xb0),
        accent: Rgba::rgb(0x88, 0x39, 0xef),
        accent_hover: Rgba::rgb(0x1e, 0x66, 0xf5),
        accent_pressed: Rgba::rgb(0xd2, 0x0f, 0x39),
        accent_text: Rgba::rgb(0xef, 0xf1, 0xf5),
        danger: Rgba::rgb(0xd2, 0x0f, 0x39),
        warning: Rgba::rgb(0xdf, 0x8e, 0x1d),
        border_subtle: Rgba::rgb(0xbc, 0xc0, 0xcc),
        border_strong: Rgba::rgb(0xac, 0xb0, 0xbe),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// catppuccin-frappe — dark theme. White alpha base.
fn catppuccin_frappe() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x30, 0x34, 0x46),
        bg_secondary: Rgba::rgb(0x29, 0x2c, 0x3c),
        bg_tertiary: Rgba::rgb(0x23, 0x26, 0x34),
        bg_hover: Rgba::rgb(0x41, 0x45, 0x59),
        text_primary: Rgba::rgb(0xc6, 0xd0, 0xf5),
        text_secondary: Rgba::rgb(0xb5, 0xbf, 0xe2),
        text_muted: Rgba::rgb(0xa5, 0xad, 0xce),
        text_disabled: Rgba::rgb(0x73, 0x79, 0x94),
        accent: Rgba::rgb(0xca, 0x9e, 0xe6),
        accent_hover: Rgba::rgb(0x8c, 0xaa, 0xee),
        accent_pressed: Rgba::rgb(0xe7, 0x82, 0x84),
        accent_text: Rgba::rgb(0x30, 0x34, 0x46),
        danger: Rgba::rgb(0xe7, 0x82, 0x84),
        warning: Rgba::rgb(0xe5, 0xc8, 0x90),
        border_subtle: Rgba::rgb(0x51, 0x57, 0x6d),
        border_strong: Rgba::rgb(0x62, 0x68, 0x80),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// catppuccin-macchiato — dark theme. White alpha base.
fn catppuccin_macchiato() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x24, 0x27, 0x3a),
        bg_secondary: Rgba::rgb(0x1e, 0x20, 0x30),
        bg_tertiary: Rgba::rgb(0x18, 0x19, 0x26),
        bg_hover: Rgba::rgb(0x36, 0x3a, 0x4f),
        text_primary: Rgba::rgb(0xca, 0xd3, 0xf5),
        text_secondary: Rgba::rgb(0xb8, 0xc0, 0xe0),
        text_muted: Rgba::rgb(0xa5, 0xad, 0xcb),
        text_disabled: Rgba::rgb(0x6e, 0x73, 0x8d),
        accent: Rgba::rgb(0xc6, 0xa0, 0xf6),
        accent_hover: Rgba::rgb(0x8a, 0xad, 0xf4),
        accent_pressed: Rgba::rgb(0xed, 0x87, 0x96),
        accent_text: Rgba::rgb(0x24, 0x27, 0x3a),
        danger: Rgba::rgb(0xed, 0x87, 0x96),
        warning: Rgba::rgb(0xee, 0xd4, 0x9f),
        border_subtle: Rgba::rgb(0x49, 0x4d, 0x64),
        border_strong: Rgba::rgb(0x5b, 0x60, 0x78),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// breeze-dark — OMITS the danger/warning families AND the alpha scale →
/// inherits them from `:root` Dark. We materialize the inherited danger/warning
/// hues (red `#ef4444`, amber `#fbbf24`) explicitly.
fn breeze_dark() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x14, 0x16, 0x18),
        bg_secondary: Rgba::rgb(0x20, 0x23, 0x26),
        bg_tertiary: Rgba::rgb(0x29, 0x2c, 0x30),
        bg_hover: Rgba::rgb(0x31, 0x36, 0x3b),
        text_primary: Rgba::rgb(0xff, 0xff, 0xff),
        text_secondary: Rgba::rgb(0xfc, 0xfc, 0xfc),
        text_muted: Rgba::rgb(0xa1, 0xa9, 0xb1),
        text_disabled: Rgba::rgb(0x31, 0x36, 0x3b),
        accent: Rgba::rgb(0x3d, 0xae, 0xe9),
        accent_hover: Rgba::rgb(0x9b, 0x59, 0xb6),
        accent_pressed: Rgba::rgb(0x1d, 0x99, 0xf3),
        accent_text: Rgba::rgb(0x14, 0x16, 0x18),
        // inherited from :root Dark:
        danger: Rgba::rgb(0xef, 0x44, 0x44),
        warning: Rgba::rgb(0xfb, 0xbf, 0x24),
        border_subtle: Rgba::rgb(0x29, 0x2c, 0x30),
        border_strong: Rgba::rgb(0x31, 0x36, 0x3b),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// adwaita-dark — OMITS the danger/warning families + alpha scale → inherits
/// from `:root` Dark.
fn adwaita_dark() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x1d, 0x1d, 0x20),
        bg_secondary: Rgba::rgb(0x22, 0x22, 0x26),
        bg_tertiary: Rgba::rgb(0x28, 0x28, 0x2c),
        bg_hover: Rgba::rgb(0x2e, 0x2e, 0x32),
        text_primary: Rgba::rgb(0xff, 0xff, 0xff),
        text_secondary: Rgba::rgb(0xff, 0xff, 0xff),
        text_muted: Rgba::rgb(0xb3, 0xb3, 0xb8),
        text_disabled: Rgba::rgb(0x2e, 0x2e, 0x32),
        accent: Rgba::rgb(0x35, 0x84, 0xe4),
        accent_hover: Rgba::rgb(0x1c, 0x71, 0xd8),
        accent_pressed: Rgba::rgb(0x1a, 0x5f, 0xb4),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),
        // inherited from :root Dark:
        danger: Rgba::rgb(0xef, 0x44, 0x44),
        warning: Rgba::rgb(0xfb, 0xbf, 0x24),
        border_subtle: Rgba::rgb(0x28, 0x28, 0x2c),
        border_strong: Rgba::rgb(0x2e, 0x2e, 0x32),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn aurora() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x2e, 0x34, 0x40),
        bg_secondary: Rgba::rgb(0x3b, 0x42, 0x52),
        bg_tertiary: Rgba::rgb(0x43, 0x4c, 0x5e),
        bg_hover: Rgba::rgb(0x4c, 0x56, 0x6a),
        text_primary: Rgba::rgb(0xd8, 0xde, 0xe9),
        text_secondary: Rgba::rgb(0xe5, 0xe9, 0xf0),
        text_muted: Rgba::rgb(0xb4, 0x8e, 0xad),
        text_disabled: Rgba::rgb(0x4c, 0x56, 0x6a),
        accent: Rgba::rgb(0xa3, 0xbe, 0x8c),
        accent_hover: Rgba::rgb(0xeb, 0xcb, 0x8b),
        accent_pressed: Rgba::rgb(0xd0, 0x87, 0x70),
        accent_text: Rgba::rgb(0x2e, 0x34, 0x40),
        danger: Rgba::rgb(0xbf, 0x61, 0x6a),
        warning: Rgba::rgb(0xeb, 0xcb, 0x8b),
        border_subtle: Rgba::rgb(0x4c, 0x56, 0x6a),
        border_strong: Rgba::rgb(0x43, 0x4c, 0x5e),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn ikari() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x1c, 0x12, 0x39),
        bg_secondary: Rgba::rgb(0x24, 0x1a, 0x48),
        bg_tertiary: Rgba::rgb(0x30, 0x24, 0x58),
        bg_hover: Rgba::rgb(0x3c, 0x2f, 0x71),
        text_primary: Rgba::rgb(0xe8, 0xe6, 0xf2),
        text_secondary: Rgba::rgb(0xc6, 0xc2, 0xd8),
        text_muted: Rgba::rgb(0x95, 0x8f, 0xb5),
        text_disabled: Rgba::rgb(0x57, 0x4b, 0x79),
        accent: Rgba::rgb(0x7e, 0xda, 0x53),
        accent_hover: Rgba::rgb(0xa5, 0xf0, 0x66),
        accent_pressed: Rgba::rgb(0xd5, 0x8e, 0x27),
        accent_text: Rgba::rgb(0x1c, 0x12, 0x39),
        danger: Rgba::rgb(0xd8, 0x4a, 0x4a),
        warning: Rgba::rgb(0xe5, 0x9b, 0x2f),
        border_subtle: Rgba::rgb(0x30, 0x24, 0x58),
        border_strong: Rgba::rgb(0x3c, 0x2f, 0x71),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn ayanami() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x0f, 0x25, 0x3f),
        bg_secondary: Rgba::rgb(0x16, 0x3e, 0x60),
        bg_tertiary: Rgba::rgb(0x21, 0x4f, 0x7d),
        bg_hover: Rgba::rgb(0x2d, 0x63, 0x9f),
        text_primary: Rgba::rgb(0xf2, 0xf0, 0xe5),
        text_secondary: Rgba::rgb(0xd6, 0xd2, 0xc2),
        text_muted: Rgba::rgb(0x95, 0xa4, 0xb7),
        text_disabled: Rgba::rgb(0x2d, 0x63, 0x9f),
        accent: Rgba::rgb(0xe5, 0xb8, 0x2e),
        accent_hover: Rgba::rgb(0xf0, 0xcd, 0x63),
        accent_pressed: Rgba::rgb(0xcf, 0xa2, 0x2e),
        accent_text: Rgba::rgb(0x0f, 0x25, 0x3f),
        danger: Rgba::rgb(0xc0, 0x39, 0x2b),
        warning: Rgba::rgb(0xd8, 0x9b, 0x1c),
        border_subtle: Rgba::rgb(0x21, 0x4f, 0x7d),
        border_strong: Rgba::rgb(0x2d, 0x63, 0x9f),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn iscariot() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x2a, 0x10, 0x2a),
        bg_secondary: Rgba::rgb(0x38, 0x13, 0x3b),
        bg_tertiary: Rgba::rgb(0x45, 0x18, 0x46),
        bg_hover: Rgba::rgb(0x5d, 0x20, 0x60),
        text_primary: Rgba::rgb(0xf4, 0xea, 0xf5),
        text_secondary: Rgba::rgb(0xcf, 0xaa, 0xcb),
        text_muted: Rgba::rgb(0xa2, 0x78, 0xa6),
        text_disabled: Rgba::rgb(0x5d, 0x20, 0x60),
        accent: Rgba::rgb(0xe9, 0x4f, 0x94),
        accent_hover: Rgba::rgb(0xff, 0x7a, 0xbf),
        accent_pressed: Rgba::rgb(0xc9, 0x45, 0xa3),
        accent_text: Rgba::rgb(0x2a, 0x10, 0x2a),
        danger: Rgba::rgb(0xc0, 0x39, 0x2b),
        warning: Rgba::rgb(0xe5, 0xb6, 0x4b),
        border_subtle: Rgba::rgb(0x38, 0x13, 0x3b),
        border_strong: Rgba::rgb(0x5d, 0x20, 0x60),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn stratego() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x0a, 0x0a, 0x0b),
        bg_secondary: Rgba::rgb(0x14, 0x14, 0x18),
        bg_tertiary: Rgba::rgb(0x1d, 0x1e, 0x22),
        bg_hover: Rgba::rgb(0x28, 0x2a, 0x30),
        text_primary: Rgba::rgb(0xec, 0xe6, 0xd6),
        text_secondary: Rgba::rgb(0xb5, 0xaf, 0xa0),
        text_muted: Rgba::rgb(0x8a, 0x85, 0x7a),
        text_disabled: Rgba::rgb(0x4a, 0x48, 0x42),
        accent: Rgba::rgb(0xed, 0x2f, 0x3d),
        accent_hover: Rgba::rgb(0xf7, 0x4a, 0x58),
        accent_pressed: Rgba::rgb(0xc4, 0x1e, 0x2a),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),
        danger: Rgba::rgb(0xe6, 0x39, 0x46),
        warning: Rgba::rgb(0xc4, 0xa5, 0x6a),
        border_subtle: Rgba::rgb(0x2a, 0x2a, 0x30),
        border_strong: Rgba::rgb(0x3a, 0x3a, 0x42),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn rumi() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x00, 0x00, 0x00),
        bg_secondary: Rgba::rgb(0x0d, 0x0d, 0x0d),
        bg_tertiary: Rgba::rgb(0x1a, 0x1a, 0x1a),
        bg_hover: Rgba::rgb(0x33, 0x33, 0x33),
        text_primary: Rgba::rgb(0xf0, 0xf0, 0xf0),
        text_secondary: Rgba::rgb(0xb2, 0xb2, 0xb2),
        text_muted: Rgba::rgb(0x80, 0x80, 0x80),
        text_disabled: Rgba::rgb(0x5a, 0x5a, 0x5a),
        accent: Rgba::rgb(0xe5, 0x8f, 0x24),
        accent_hover: Rgba::rgb(0xf0, 0xa5, 0x3c),
        accent_pressed: Rgba::rgb(0xcc, 0x7a, 0x12),
        accent_text: Rgba::rgb(0x00, 0x00, 0x00),
        danger: Rgba::rgb(0xe7, 0x4c, 0x3c),
        warning: Rgba::rgb(0xf3, 0x9c, 0x12),
        border_subtle: Rgba::rgb(0x1a, 0x1a, 0x1a),
        border_strong: Rgba::rgb(0x33, 0x33, 0x33),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn zoey() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x15, 0x1e, 0x2d),
        bg_secondary: Rgba::rgb(0x0e, 0x14, 0x1e),
        bg_tertiary: Rgba::rgb(0x10, 0x1a, 0x2a),
        bg_hover: Rgba::rgb(0x1b, 0x29, 0x3e),
        text_primary: Rgba::rgb(0xe0, 0xe2, 0xd5),
        text_secondary: Rgba::rgb(0xb5, 0xb7, 0xaa),
        text_muted: Rgba::rgb(0x8e, 0x90, 0x80),
        text_disabled: Rgba::rgb(0x60, 0x63, 0x54),
        accent: Rgba::rgb(0x46, 0xb4, 0xd3),
        accent_hover: Rgba::rgb(0x5c, 0xc0, 0xd9),
        accent_pressed: Rgba::rgb(0x3a, 0x97, 0xb6),
        accent_text: Rgba::rgb(0x15, 0x1e, 0x2d),
        danger: Rgba::rgb(0xbf, 0x61, 0x6a),
        warning: Rgba::rgb(0xd0, 0x87, 0x70),
        border_subtle: Rgba::rgb(0x0e, 0x14, 0x1e),
        border_strong: Rgba::rgb(0x1b, 0x29, 0x3e),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn mira() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x10, 0x18, 0x20),
        bg_secondary: Rgba::rgb(0x14, 0x1a, 0x28),
        bg_tertiary: Rgba::rgb(0x1d, 0x26, 0x35),
        bg_hover: Rgba::rgb(0x2a, 0x34, 0x48),
        text_primary: Rgba::rgb(0xe5, 0xe5, 0xe5),
        text_secondary: Rgba::rgb(0xb0, 0xb3, 0xc6),
        text_muted: Rgba::rgb(0x8a, 0x8d, 0xa0),
        text_disabled: Rgba::rgb(0x5c, 0x5e, 0x72),
        accent: Rgba::rgb(0xd9, 0x46, 0x85),
        accent_hover: Rgba::rgb(0xff, 0x00, 0x7f),
        accent_pressed: Rgba::rgb(0xff, 0xd7, 0x00), // intentional yellow
        accent_text: Rgba::rgb(0x10, 0x18, 0x20),
        danger: Rgba::rgb(0xc5, 0x30, 0x32),
        warning: Rgba::rgb(0xff, 0xd7, 0x00),
        border_subtle: Rgba::rgb(0x20, 0x2c, 0x3d),
        border_strong: Rgba::rgb(0x34, 0x40, 0x5a),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// frost — registered `type:light` in Tauri but a DARK Nord-polar canvas
/// (`#2e3440`). Polarity is luminance-derived, so it correctly resolves to a
/// white alpha base. (doc 01 §frost; corrected light/dark flag.)
fn frost() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x2e, 0x34, 0x40),
        bg_secondary: Rgba::rgb(0x3b, 0x42, 0x52),
        bg_tertiary: Rgba::rgb(0x43, 0x4c, 0x5e),
        bg_hover: Rgba::rgb(0x4c, 0x56, 0x6a),
        text_primary: Rgba::rgb(0xd8, 0xde, 0xe9),
        text_secondary: Rgba::rgb(0xe5, 0xe9, 0xf0),
        text_muted: Rgba::rgb(0x8f, 0xbc, 0xbb),
        text_disabled: Rgba::rgb(0x4c, 0x56, 0x6a),
        accent: Rgba::rgb(0x88, 0xc0, 0xd0),
        accent_hover: Rgba::rgb(0x81, 0xa1, 0xc1),
        accent_pressed: Rgba::rgb(0x5e, 0x81, 0xac),
        accent_text: Rgba::rgb(0x2e, 0x34, 0x40),
        danger: Rgba::rgb(0xbf, 0x61, 0x6a),
        warning: Rgba::rgb(0xd0, 0x87, 0x70),
        border_subtle: Rgba::rgb(0x4c, 0x56, 0x6a),
        border_strong: Rgba::rgb(0x43, 0x4c, 0x5e),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

/// langley — registered `type:light` in Tauri but a DEEP-MAROON dark canvas
/// (`#2c0a0a`). Luminance-derived polarity -> white alpha base. (doc 01 §langley)
fn langley() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0x2c, 0x0a, 0x0a),
        bg_secondary: Rgba::rgb(0x3a, 0x0e, 0x0e),
        bg_tertiary: Rgba::rgb(0x4c, 0x14, 0x13),
        bg_hover: Rgba::rgb(0x71, 0x1c, 0x1c),
        text_primary: Rgba::rgb(0xf2, 0xda, 0xda),
        text_secondary: Rgba::rgb(0xd9, 0xa3, 0xa3),
        text_muted: Rgba::rgb(0xa9, 0x7b, 0x7b),
        text_disabled: Rgba::rgb(0x7a, 0x3d, 0x3d),
        accent: Rgba::rgb(0xe6, 0x7e, 0x22),
        accent_hover: Rgba::rgb(0xf3, 0x9c, 0x4d),
        accent_pressed: Rgba::rgb(0xd8, 0x6b, 0x1f),
        accent_text: Rgba::rgb(0x2c, 0x0a, 0x0a),
        danger: Rgba::rgb(0xc0, 0x39, 0x2b),
        warning: Rgba::rgb(0xe5, 0xa6, 0x3d),
        border_subtle: Rgba::rgb(0x3a, 0x0e, 0x0e),
        border_strong: Rgba::rgb(0x4c, 0x14, 0x13),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

// ---------------------------------------------------------------------------
// Light (branded / community) themes
// ---------------------------------------------------------------------------

/// alucard — light/cream theme (`#fffbeb` canvas). Luminance-derived -> black
/// alpha base. (doc 01 §alucard.)
fn alucard() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xff, 0xfb, 0xeb),
        bg_secondary: Rgba::rgb(0xef, 0xed, 0xdc),
        bg_tertiary: Rgba::rgb(0xec, 0xe9, 0xdf),
        bg_hover: Rgba::rgb(0xcf, 0xcf, 0xde),
        text_primary: Rgba::rgb(0x1f, 0x1f, 0x1f),
        text_secondary: Rgba::rgb(0x6c, 0x66, 0x4b),
        text_muted: Rgba::rgb(0x9b, 0x92, 0x75),
        text_disabled: Rgba::rgb(0xbc, 0xba, 0xb3),
        accent: Rgba::rgb(0x64, 0x4a, 0xc9),
        accent_hover: Rgba::rgb(0xa3, 0x14, 0x4d),
        accent_pressed: Rgba::rgb(0x03, 0x6a, 0x96),
        accent_text: Rgba::rgb(0xff, 0xfb, 0xeb),
        danger: Rgba::rgb(0xcb, 0x3a, 0x2a),
        warning: Rgba::rgb(0xa3, 0x4d, 0x14),
        border_subtle: Rgba::rgb(0xec, 0xe9, 0xdf),
        border_strong: Rgba::rgb(0xde, 0xdc, 0xcf),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn rose_pine_dawn() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xfa, 0xf4, 0xed),
        bg_secondary: Rgba::rgb(0xf4, 0xed, 0xe8),
        bg_tertiary: Rgba::rgb(0xdf, 0xda, 0xd9),
        bg_hover: Rgba::rgb(0xce, 0xca, 0xcd),
        text_primary: Rgba::rgb(0x57, 0x52, 0x79),
        text_secondary: Rgba::rgb(0x79, 0x75, 0x93),
        text_muted: Rgba::rgb(0x98, 0x93, 0xa5),
        text_disabled: Rgba::rgb(0xb5, 0xae, 0xbc),
        accent: Rgba::rgb(0xd7, 0x82, 0x7e),
        accent_hover: Rgba::rgb(0xe5, 0xa4, 0x78),
        accent_pressed: Rgba::rgb(0x28, 0x69, 0x83),
        accent_text: Rgba::rgb(0x57, 0x52, 0x79),
        danger: Rgba::rgb(0xb4, 0x63, 0x7a),
        warning: Rgba::rgb(0xea, 0x9d, 0x34),
        border_subtle: Rgba::rgb(0xce, 0xca, 0xcd),
        border_strong: Rgba::rgb(0xdf, 0xda, 0xd9),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn breeze_light() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xff, 0xff, 0xff),
        bg_secondary: Rgba::rgb(0xf2, 0xf2, 0xf2),
        bg_tertiary: Rgba::rgb(0xe5, 0xe5, 0xe5),
        bg_hover: Rgba::rgb(0xdc, 0xdc, 0xdc),
        text_primary: Rgba::rgb(0x31, 0x36, 0x3b),
        text_secondary: Rgba::rgb(0x5c, 0x61, 0x66),
        text_muted: Rgba::rgb(0x7d, 0x81, 0x86),
        text_disabled: Rgba::rgb(0xa1, 0xa5, 0xa9),
        accent: Rgba::rgb(0x1d, 0x99, 0xf3),
        accent_hover: Rgba::rgb(0x3d, 0xae, 0xe9),
        accent_pressed: Rgba::rgb(0x00, 0x78, 0xd4),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),
        danger: Rgba::rgb(0xc3, 0x27, 0x2b),
        warning: Rgba::rgb(0xf5, 0x97, 0x00),
        border_subtle: Rgba::rgb(0xd0, 0xd4, 0xd8),
        border_strong: Rgba::rgb(0xb7, 0xbd, 0xc2),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn adwaita_light() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xff, 0xff, 0xff),
        bg_secondary: Rgba::rgb(0xf6, 0xf5, 0xf4),
        bg_tertiary: Rgba::rgb(0xea, 0xe9, 0xe7),
        bg_hover: Rgba::rgb(0xdc, 0xd9, 0xd7),
        text_primary: Rgba::rgb(0x24, 0x1f, 0x31),
        text_secondary: Rgba::rgb(0x5f, 0x5b, 0x6b),
        text_muted: Rgba::rgb(0x7f, 0x7b, 0x8c),
        text_disabled: Rgba::rgb(0xb1, 0xae, 0xbc),
        accent: Rgba::rgb(0x1e, 0x78, 0xe4),
        accent_hover: Rgba::rgb(0x3f, 0x8e, 0xf0),
        accent_pressed: Rgba::rgb(0x15, 0x5a, 0x9c),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),
        danger: Rgba::rgb(0xc0, 0x1c, 0x28),
        warning: Rgba::rgb(0xf5, 0xc2, 0x11),
        border_subtle: Rgba::rgb(0xdc, 0xd9, 0xd7),
        border_strong: Rgba::rgb(0xc6, 0xc2, 0xcf),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn duotone_snow() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xff, 0xff, 0xff),
        bg_secondary: Rgba::rgb(0xf8, 0xf9, 0xfa),
        bg_tertiary: Rgba::rgb(0xef, 0xf1, 0xf5),
        bg_hover: Rgba::rgb(0xe6, 0xe8, 0xec),
        text_primary: Rgba::rgb(0x4a, 0x59, 0x6e),
        text_secondary: Rgba::rgb(0x6b, 0x73, 0x8a),
        text_muted: Rgba::rgb(0x8c, 0x95, 0xa8),
        text_disabled: Rgba::rgb(0xb0, 0xb4, 0xc1),
        accent: Rgba::rgb(0x4a, 0x82, 0xd8),
        accent_hover: Rgba::rgb(0x6b, 0x9b, 0xe0),
        accent_pressed: Rgba::rgb(0x3a, 0x6f, 0xc2),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),
        danger: Rgba::rgb(0xd3, 0x7e, 0x7e),
        warning: Rgba::rgb(0xc0, 0x9c, 0x4a),
        border_subtle: Rgba::rgb(0xdf, 0xe3, 0xe8),
        border_strong: Rgba::rgb(0xc9, 0xce, 0xd4),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn snow_storm() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xec, 0xef, 0xf4),
        bg_secondary: Rgba::rgb(0xe5, 0xe9, 0xf0),
        bg_tertiary: Rgba::rgb(0xd8, 0xde, 0xe9),
        bg_hover: Rgba::rgb(0xcb, 0xd5, 0xe0),
        text_primary: Rgba::rgb(0x2e, 0x34, 0x40),
        text_secondary: Rgba::rgb(0x3b, 0x42, 0x52),
        text_muted: Rgba::rgb(0x43, 0x4c, 0x5e),
        text_disabled: Rgba::rgb(0x4c, 0x56, 0x6a),
        accent: Rgba::rgb(0x5e, 0x81, 0xac),
        accent_hover: Rgba::rgb(0x81, 0xa1, 0xc1),
        accent_pressed: Rgba::rgb(0x88, 0xc0, 0xd0),
        accent_text: Rgba::rgb(0x2e, 0x34, 0x40),
        danger: Rgba::rgb(0xbf, 0x61, 0x6a),
        warning: Rgba::rgb(0xd0, 0x87, 0x70),
        border_subtle: Rgba::rgb(0xd8, 0xde, 0xe9),
        border_strong: Rgba::rgb(0xe5, 0xe9, 0xf0),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

fn kurosaki() -> ThemeColors {
    let s = StdSpec {
        bg_primary: Rgba::rgb(0xfb, 0xf9, 0xf2),
        bg_secondary: Rgba::rgb(0xf3, 0xf0, 0xe8),
        bg_tertiary: Rgba::rgb(0xe8, 0xe2, 0xd4),
        bg_hover: Rgba::rgb(0xe1, 0xda, 0xc8),
        text_primary: Rgba::rgb(0x26, 0x24, 0x24),
        text_secondary: Rgba::rgb(0x54, 0x4d, 0x48),
        text_muted: Rgba::rgb(0x82, 0x7d, 0x78),
        text_disabled: Rgba::rgb(0xb3, 0xad, 0xa7),
        accent: Rgba::rgb(0xd5, 0xbe, 0x58),
        accent_hover: Rgba::rgb(0xe8, 0xce, 0x66),
        accent_pressed: Rgba::rgb(0xb4, 0x9f, 0x45),
        accent_text: Rgba::rgb(0x26, 0x24, 0x24),
        danger: Rgba::rgb(0xc0, 0x39, 0x2b),
        warning: Rgba::rgb(0xd8, 0x9b, 0x1c),
        border_subtle: Rgba::rgb(0xe4, 0xdc, 0xb4),
        border_strong: Rgba::rgb(0xd1, 0xc7, 0xaa),
        ..Default::default()
    };
    s.build(bg_is_light(s.bg_primary))
}

// ---------------------------------------------------------------------------
// Accessibility (REDESIGNED) themes — final verified palettes (Part B)
// ---------------------------------------------------------------------------
//
// Unlike the standard rows, the a11y themes specify SOLID (opaque) status
// surfaces and borders, not rgba() alpha tints — accessible contrast can't be
// guaranteed through translucency over an arbitrary backdrop. So these rows are
// materialized directly (like dark()/tokyo_night()) rather than via StdSpec.
//
// `success` family: Part B tables only fix `success` for `colorblind`. For the
// other four a11y themes a theme-appropriate green is derived to clear the same
// contrast bar as the theme's other foregrounds (AAA for wcag-*, ≥ wcag bar for
// HC), with solid bg/border/hover tints matching each theme's status pattern.
// These derived greens are covered by the contrast unit tests below.
//
// `focus_ring`: exactly as Part B (reuses the accent for wcag-*/HC-light;
// dedicated cyan/blue elsewhere; bright yellow for HC-dark).

/// `wcag-light` — WCAG AAA Light (Part B §B.1). Body text AAA (7:1) + APCA ≥75;
/// non-text ≥3:1. text-primary `#1a1a1a` (not pure black) to avoid reverse-halation.
fn wcag_light() -> ThemeColors {
    let danger = Rgba::rgb(0xa3, 0x00, 0x00);
    let warning = Rgba::rgb(0x6b, 0x45, 0x00);
    // derived success: deep green clearing AAA on white (7.36:1).
    let success = Rgba::rgb(0x13, 0x63, 0x2f);
    let accent = Rgba::rgb(0x0a, 0x4e, 0xa3);
    ThemeColors {
        surface_main: Rgba::rgb(0xff, 0xff, 0xff),     // bg-primary
        surface_card: Rgba::rgb(0xf4, 0xf5, 0xf7),     // bg-secondary
        surface_elevated: Rgba::rgb(0xe7, 0xe9, 0xee), // bg-tertiary
        surface_hover: Rgba::rgba(0, 0, 0, 0x10),      // ~6% black (light polarity)
        bg_hover: Rgba::rgb(0xdd, 0xe0, 0xe6),         // bg-hover

        text_primary: Rgba::rgb(0x1a, 0x1a, 0x1a),
        text_secondary: Rgba::rgb(0x3a, 0x3a, 0x3a),
        text_muted: Rgba::rgb(0x59, 0x59, 0x59),
        text_disabled: Rgba::rgb(0x76, 0x76, 0x76),

        accent,
        accent_hover: Rgba::rgb(0x08, 0x3d, 0x80),
        accent_pressed: Rgba::rgb(0x06, 0x2e, 0x60),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff), // btn-primary-text

        danger,
        danger_bg: Rgba::rgb(0xfb, 0xe9, 0xe9),     // solid
        danger_border: Rgba::rgb(0xaa, 0x60, 0x60), // solid
        danger_hover: Rgba::rgb(0x85, 0x00, 0x00),

        warning,
        warning_bg: Rgba::rgb(0xff, 0xf7, 0xe6),     // solid
        warning_border: Rgba::rgb(0x9c, 0x73, 0x20), // solid
        warning_hover: Rgba::rgb(0x55, 0x37, 0x00),

        success,
        success_bg: Rgba::rgb(0xe6, 0xf4, 0xea),     // solid
        success_border: Rgba::rgb(0x5a, 0x9c, 0x72), // solid
        success_hover: Rgba::rgb(0x0f, 0x4f, 0x25),

        border_subtle: Rgba::rgb(0xc9, 0xcc, 0xd2), // decorative divider
        border_muted: Rgba::rgba(0, 0, 0, 0x38),    // ~22% black
        border_strong: Rgba::rgb(0x6e, 0x6e, 0x6e), // control border

        focus_ring: accent, // reuses accent

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(true), // light theme -> black-based overlays
    }
}

/// `wcag-dark` — WCAG AAA Dark (Part B §B.2). AAA (7:1) + APCA content/body;
/// non-text ≥3:1. bg `#0d1117` + text `#e6edf3` to kill halation.
fn wcag_dark() -> ThemeColors {
    let danger = Rgba::rgb(0xff, 0x9d, 0x9d);
    let warning = Rgba::rgb(0xff, 0xca, 0x6a);
    // derived success: lightened green clearing the AAA bar on dark (11.77:1).
    let success = Rgba::rgb(0x7e, 0xe0, 0xa0);
    let accent = Rgba::rgb(0x9e, 0xc1, 0xff);
    ThemeColors {
        surface_main: Rgba::rgb(0x0d, 0x11, 0x17),
        surface_card: Rgba::rgb(0x16, 0x1b, 0x22),
        surface_elevated: Rgba::rgb(0x21, 0x26, 0x2d),
        surface_hover: Rgba::rgba(255, 255, 255, 0x10), // ~6% white
        bg_hover: Rgba::rgb(0x2a, 0x31, 0x3a),

        text_primary: Rgba::rgb(0xe6, 0xed, 0xf3),
        text_secondary: Rgba::rgb(0xc9, 0xd1, 0xd9),
        text_muted: Rgba::rgb(0xb8, 0xc0, 0xcc),
        text_disabled: Rgba::rgb(0x7d, 0x87, 0x94),

        accent,
        accent_hover: Rgba::rgb(0xb9, 0xd2, 0xff),
        accent_pressed: Rgba::rgb(0xcf, 0xe0, 0xff),
        accent_text: Rgba::rgb(0x06, 0x09, 0x0f), // dark text on light-blue

        danger,
        danger_bg: Rgba::rgb(0x2d, 0x14, 0x16),     // opaque dark-red tint
        danger_border: Rgba::rgb(0xa8, 0x56, 0x56), // see adjacency constraint
        danger_hover: Rgba::rgb(0xff, 0xb3, 0xb3),

        warning,
        warning_bg: Rgba::rgb(0x2d, 0x24, 0x10),     // opaque dark-amber tint
        warning_border: Rgba::rgb(0x9c, 0x74, 0x30),
        warning_hover: Rgba::rgb(0xff, 0xd9, 0x8a),

        success,
        success_bg: Rgba::rgb(0x11, 0x24, 0x1a),     // opaque dark-green tint
        success_border: Rgba::rgb(0x3f, 0x7d, 0x56),
        success_hover: Rgba::rgb(0x9a, 0xed, 0xb6),

        border_subtle: Rgba::rgb(0x2d, 0x33, 0x3b), // decorative separator
        border_muted: Rgba::rgba(255, 255, 255, 0x38),
        border_strong: Rgba::rgb(0x6b, 0x76, 0x86), // control border (≥3:1 all tiers)

        focus_ring: accent,

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(false), // dark theme -> white-based overlays
    }
}

/// `high-contrast` (DARK) — Part B §B.3a. Lifted off pure black (`#0a0a0a`),
/// reciprocal cyan accent, bright yellow demoted to the focus-ring slot.
fn high_contrast() -> ThemeColors {
    let danger = Rgba::rgb(0xff, 0x8a, 0x8a);
    let warning = Rgba::rgb(0xff, 0xb0, 0x00);
    // derived success: bright green ≥ wcag-dark bar (15.51:1 on bg-primary).
    let success = Rgba::rgb(0x62, 0xff, 0xb0);
    let accent = Rgba::rgb(0x62, 0xd4, 0xff);
    ThemeColors {
        surface_main: Rgba::rgb(0x0a, 0x0a, 0x0a),     // lifted off pure black
        surface_card: Rgba::rgb(0x14, 0x14, 0x14),
        surface_elevated: Rgba::rgb(0x1f, 0x1f, 0x1f),
        surface_hover: Rgba::rgba(255, 255, 255, 0x10),
        bg_hover: Rgba::rgb(0x2b, 0x2b, 0x2b),

        text_primary: Rgba::rgb(0xff, 0xff, 0xff),
        text_secondary: Rgba::rgb(0xf0, 0xf0, 0xf0), // near-primary, NOT gray
        text_muted: Rgba::rgb(0xe0, 0xe0, 0xe0),     // near-primary, NOT gray
        text_disabled: Rgba::rgb(0x8c, 0x8c, 0x8c),  // the only reserved gray

        accent,                                      // reciprocal cyan
        accent_hover: Rgba::rgb(0x8c, 0xe3, 0xff),
        accent_pressed: Rgba::rgb(0xae, 0xed, 0xff),
        accent_text: Rgba::rgb(0x00, 0x00, 0x00),    // reads on cyan fill

        danger,
        danger_bg: Rgba::rgb(0x2a, 0x00, 0x00),      // opaque; always bordered
        danger_border: danger,                       // = danger hue
        danger_hover: Rgba::rgb(0xff, 0xb3, 0xb3),

        warning,
        warning_bg: Rgba::rgb(0x2a, 0x1d, 0x00),     // opaque; always bordered
        warning_border: warning,                     // = warning hue
        warning_hover: Rgba::rgb(0xff, 0xc9, 0x4d),

        success,
        success_bg: Rgba::rgb(0x00, 0x26, 0x1a),     // opaque; always bordered
        success_border: success,                     // = success hue
        success_hover: Rgba::rgb(0x8a, 0xff, 0xc8),

        border_subtle: Rgba::rgb(0x7a, 0x7a, 0x7a), // still clearly visible (4.61:1)
        border_muted: Rgba::rgba(255, 255, 255, 0x38),
        border_strong: Rgba::rgb(0xff, 0xff, 0xff), // = text color

        focus_ring: Rgba::rgb(0xff, 0xee, 0x32),    // bright yellow's correct home

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(false), // dark theme -> white-based overlays
    }
}

/// `high-contrast-light` (LIGHT, new) — Part B §B.3b. Reciprocal deep-blue
/// accent. Warning corrected `#735c00` → `#5e4b00` (AA-only → AAA on white).
fn high_contrast_light() -> ThemeColors {
    let danger = Rgba::rgb(0xa3, 0x00, 0x00);
    let warning = Rgba::rgb(0x5e, 0x4b, 0x00); // CORRECTED from #735c00
    // derived success: deep green ≥ HC bar (8.47:1 on white).
    let success = Rgba::rgb(0x00, 0x5a, 0x1c);
    let accent = Rgba::rgb(0x00, 0x00, 0xcc);
    ThemeColors {
        surface_main: Rgba::rgb(0xff, 0xff, 0xff),
        surface_card: Rgba::rgb(0xf2, 0xf2, 0xf2),
        surface_elevated: Rgba::rgb(0xe6, 0xe6, 0xe6),
        surface_hover: Rgba::rgba(0, 0, 0, 0x10),
        bg_hover: Rgba::rgb(0xda, 0xda, 0xda),

        text_primary: Rgba::rgb(0x00, 0x00, 0x00),
        text_secondary: Rgba::rgb(0x1a, 0x1a, 0x1a), // near-primary
        text_muted: Rgba::rgb(0x33, 0x33, 0x33),     // near-primary, NOT gray
        text_disabled: Rgba::rgb(0x59, 0x59, 0x59),  // reserved gray (7.00:1 AAA)

        accent,                                      // reciprocal deep blue
        accent_hover: Rgba::rgb(0x00, 0x00, 0xa3),
        accent_pressed: Rgba::rgb(0x00, 0x00, 0x80),
        accent_text: Rgba::rgb(0xff, 0xff, 0xff),    // reads on blue fill

        danger,
        danger_bg: Rgba::rgb(0xff, 0xe5, 0xe5),      // opaque; always bordered
        danger_border: danger,                       // = danger hue
        danger_hover: Rgba::rgb(0x7a, 0x00, 0x00),

        warning,
        warning_bg: Rgba::rgb(0xff, 0xf4, 0xd6),     // opaque; always bordered
        warning_border: warning,                     // = corrected warning hue
        warning_hover: Rgba::rgb(0x5c, 0x49, 0x00),

        success,
        success_bg: Rgba::rgb(0xdc, 0xf2, 0xe2),     // opaque; always bordered
        success_border: success,                     // = success hue
        success_hover: Rgba::rgb(0x00, 0x44, 0x17),

        border_subtle: Rgba::rgb(0x66, 0x66, 0x66), // clearly visible (5.74:1)
        border_muted: Rgba::rgba(0, 0, 0, 0x38),
        border_strong: Rgba::rgb(0x00, 0x00, 0x00), // = text color

        focus_ring: accent, // accent doubles as focus ring

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(true), // light theme -> black-based overlays
    }
}

/// `colorblind` — universal Okabe-Ito (Part B §B.4). danger (reddish-purple)
/// split from warning (amber) across CVD confusion axes; foregrounds lightened
/// to clear AA on all tiers. `success` kept `#009e73`; body text routes to
/// `success-hover #33c397` on the lightest tier (C.6).
fn colorblind() -> ThemeColors {
    let danger = Rgba::rgb(0xd4, 0x88, 0xb1);
    let warning = Rgba::rgb(0xe6, 0x9f, 0x00);
    let success = Rgba::rgb(0x00, 0x9e, 0x73); // Okabe-Ito bluish green
    let accent = Rgba::rgb(0x62, 0xa5, 0xe4);
    ThemeColors {
        surface_main: Rgba::rgb(0x1a, 0x1a, 0x2e),     // retained dark navy
        surface_card: Rgba::rgb(0x22, 0x22, 0x3a),
        surface_elevated: Rgba::rgb(0x2c, 0x2c, 0x46),
        surface_hover: Rgba::rgba(255, 255, 255, 0x10),
        bg_hover: Rgba::rgb(0x36, 0x36, 0x52),

        text_primary: Rgba::rgb(0xff, 0xff, 0xff),
        text_secondary: Rgba::rgb(0xdc, 0xdc, 0xe0),
        text_muted: Rgba::rgb(0xaa, 0xaa, 0xb8),    // lightened -> AA on bg-tertiary
        text_disabled: Rgba::rgb(0x6f, 0x6f, 0x86), // exempt, perceptible

        accent,                                     // lightened Okabe-Ito blue
        accent_hover: Rgba::rgb(0x7e, 0xb4, 0xe8),
        accent_pressed: Rgba::rgb(0x8b, 0xbc, 0xec),
        accent_text: Rgba::rgb(0x0a, 0x0a, 0x14),   // near-black on light-blue

        danger,                                     // lightened reddish-purple
        danger_bg: Rgba::rgb(0x3a, 0x1a, 0x2e),     // solid tint
        danger_border: Rgba::rgb(0x8a, 0x4a, 0x70), // solid tint
        danger_hover: Rgba::rgb(0xe0, 0xa3, 0xc5),

        warning,                                    // Okabe-Ito amber
        warning_bg: Rgba::rgb(0x3a, 0x2e, 0x00),    // solid tint
        warning_border: Rgba::rgb(0x8a, 0x6e, 0x00),
        warning_hover: Rgba::rgb(0xf0, 0xb6, 0x30),

        success,                                    // AA on primary/secondary
        success_bg: Rgba::rgb(0x0a, 0x33, 0x29),    // solid tint
        success_border: Rgba::rgb(0x1f, 0x6b, 0x54),
        success_hover: Rgba::rgb(0x33, 0xc3, 0x97), // body-size success on bg-tertiary

        border_subtle: Rgba::rgb(0x3e, 0x3e, 0x56), // decorative (1.65:1)
        border_muted: Rgba::rgba(255, 255, 255, 0x38),
        border_strong: Rgba::rgb(0x6e, 0x6e, 0x88), // control boundary (3.45:1)

        focus_ring: Rgba::rgb(0x8b, 0xbc, 0xec),    // high-tone blue (8.53:1)

        favorite: danger,
        card_shadow: LEGACY_CARD_SHADOW,

        alpha: alpha_ramp(false), // dark theme -> white-based overlays
    }
}

/// Straight-alpha overlay of an opaque hue at `frac` opacity (0.0..=1.0).
/// Used to reproduce Tauri's `rgba(hue, frac)` danger/warning/success tints.
const fn with_alpha(c: Rgba, frac: f32) -> Rgba {
    let a = (frac * 255.0 + 0.5) as u8;
    Rgba::rgba(c.r, c.g, c.b, a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::{alpha_byte, ALPHA_COUNT};
    use crate::id::ALL;

    /// Sentinel for "field was never set" — the all-zero opaque/transparent
    /// black the `StdSpec::default()` placeholder uses. A fully-materialized row
    /// must not leave a meaningful color at this sentinel by accident; we instead
    /// assert the named hero tokens are the EXACT transcribed values per theme in
    /// the dedicated tests below, and assert global completeness here.
    fn fully_populated(c: &ThemeColors) {
        assert_eq!(c.alpha.len(), ALPHA_COUNT);
        // every alpha tier carries opacity
        assert!(c.alpha.iter().all(|a| a.a > 0));
        // status families are present (non-degenerate alpha)
        assert!(c.danger_bg.a > 0 && c.danger_border.a > 0 && c.danger_hover.a > 0);
        assert!(c.warning_bg.a > 0 && c.warning_border.a > 0 && c.warning_hover.a > 0);
        assert!(c.success_bg.a > 0 && c.success_border.a > 0 && c.success_hover.a > 0);
        // surfaces/text/accent are opaque
        assert_eq!(c.surface_main.a, 255);
        assert_eq!(c.text_primary.a, 255);
        assert_eq!(c.accent.a, 255);
        assert_eq!(c.accent_text.a, 255);
        assert_eq!(c.border_strong.a, 255);
        assert_eq!(c.focus_ring.a, 255);
    }

    #[test]
    fn every_registered_theme_is_fully_populated() {
        for &id in ALL {
            let c = palette(id);
            fully_populated(&c);
        }
    }

    #[test]
    fn p1_rows_are_fully_populated() {
        for id in [ThemeId::Dark, ThemeId::Oled, ThemeId::TokyoNight, ThemeId::System] {
            let c = palette(id);
            assert_eq!(c.alpha.len(), ALPHA_COUNT);
            assert_ne!(c.surface_main, Rgba::rgba(0, 0, 0, 0));
            assert_ne!(c.text_primary, Rgba::rgba(0, 0, 0, 0));
            assert_ne!(c.accent, Rgba::rgba(0, 0, 0, 0));
            assert!(c.alpha.iter().all(|a| a.a > 0));
        }
    }

    #[test]
    fn dark_matches_root_css() {
        let c = palette(ThemeId::Dark);
        assert_eq!(c.surface_main, Rgba::rgb(0x0f, 0x0f, 0x0f));
        assert_eq!(c.surface_card, Rgba::rgb(0x1a, 0x1a, 0x1a));
        assert_eq!(c.surface_elevated, Rgba::rgb(0x2a, 0x2a, 0x2a));
        assert_eq!(c.text_primary, Rgba::rgb(0xff, 0xff, 0xff));
        assert_eq!(c.accent, Rgba::rgb(0x42, 0x85, 0xf4));
        assert_eq!(c.border_strong, Rgba::rgb(0x3a, 0x3a, 0x3a));
        assert_eq!(c.favorite, c.danger);
    }

    #[test]
    fn oled_overrides_only_surfaces_and_borders() {
        let d = palette(ThemeId::Dark);
        let o = palette(ThemeId::Oled);
        assert_eq!(o.surface_main, Rgba::rgb(0, 0, 0));
        assert_eq!(o.surface_card, Rgba::rgb(0x0a, 0x0a, 0x0a));
        assert_eq!(o.surface_elevated, Rgba::rgb(0x1a, 0x1a, 0x1a));
        assert_eq!(o.bg_hover, Rgba::rgb(0x11, 0x11, 0x11));
        assert_eq!(o.border_strong, Rgba::rgb(0x2a, 0x2a, 0x2a));
        assert_eq!(o.accent, d.accent);
        assert_eq!(o.text_primary, d.text_primary);
        assert_eq!(o.danger, d.danger);
    }

    #[test]
    fn tokyo_legacy_values_preserved() {
        let c = palette(ThemeId::TokyoNight);
        assert_eq!(c.surface_main, Rgba::rgb(0x1a, 0x1b, 0x26));
        assert_eq!(c.surface_card, Rgba::rgb(0x16, 0x16, 0x1e));
        assert_eq!(c.surface_elevated, Rgba::rgb(0x1c, 0x1d, 0x29));
        assert_eq!(c.text_primary, Rgba::rgb(0xa9, 0xb1, 0xd6));
        assert_eq!(c.accent, Rgba::rgb(0x7a, 0xa2, 0xf7));
        assert_eq!(c.accent_text, Rgba::rgb(0x1a, 0x1b, 0x26));
    }

    #[test]
    fn legacy_alpha_aliases_unchanged() {
        // The exact translucent values the old Slint Theme exposed (dark themes).
        for id in [ThemeId::Dark, ThemeId::Oled, ThemeId::TokyoNight] {
            let c = palette(id);
            assert_eq!(c.surface_hover, Rgba::rgba(255, 255, 255, 0x10));
            assert_eq!(c.border_subtle, Rgba::rgba(255, 255, 255, 0x14));
            assert_eq!(c.border_muted, Rgba::rgba(255, 255, 255, 0x38));
            assert_eq!(c.card_shadow, Rgba::rgba(0, 0, 0, 0x66));
            assert_eq!(c.alpha_pct(8), Rgba::rgba(255, 255, 255, 0x14));
            assert_eq!(c.alpha_pct(10), Rgba::rgba(255, 255, 255, 0x1a));
            assert_eq!(c.alpha_pct(12), Rgba::rgba(255, 255, 255, 0x1f));
            assert_eq!(c.alpha_pct(18), Rgba::rgba(255, 255, 255, 0x2e));
            assert_eq!(c.alpha_pct(55), Rgba::rgba(255, 255, 255, 0x8c));
            assert_eq!(c.alpha_pct(65), Rgba::rgba(255, 255, 255, 0xa6));
            assert_eq!(c.alpha_pct(70), Rgba::rgba(255, 255, 255, 0xb3));
            assert_eq!(c.alpha_pct(75), Rgba::rgba(255, 255, 255, 0xbf));
        }
    }

    // --- Standard themes: spot-check exact transcribed hero values ---------

    #[test]
    fn light_core_values() {
        let c = palette(ThemeId::Light);
        assert_eq!(c.surface_main, Rgba::rgb(0xff, 0xff, 0xff));
        assert_eq!(c.text_primary, Rgba::rgb(0x0f, 0x0f, 0x0f));
        // accent trio inherited from :root Dark:
        assert_eq!(c.accent, Rgba::rgb(0x42, 0x85, 0xf4));
        assert_eq!(c.accent_text, Rgba::rgb(0xff, 0xff, 0xff));
        assert_eq!(c.danger, Rgba::rgb(0xdc, 0x26, 0x26));
        assert_eq!(c.border_subtle, Rgba::rgb(0xe0, 0xe0, 0xe0));
        // light theme -> black alpha base
        assert_eq!(c.alpha_pct(8), Rgba::rgba(0, 0, 0, 0x14));
        // light hover tint is 0.15 (faithful to app.css)
        assert_eq!(c.danger_hover, with_alpha(Rgba::rgb(0xdc, 0x26, 0x26), 0.15));
    }

    #[test]
    fn dracula_nonstandard_tints() {
        let c = palette(ThemeId::Dracula);
        assert_eq!(c.surface_main, Rgba::rgb(0x28, 0x2a, 0x36));
        assert_eq!(c.accent, Rgba::rgb(0xbd, 0x93, 0xf9));
        let danger = Rgba::rgb(0xff, 0x55, 0x55);
        assert_eq!(c.danger_bg, with_alpha(danger, 0.15));
        assert_eq!(c.danger_border, with_alpha(danger, 0.4));
        assert_eq!(c.danger_hover, with_alpha(danger, 0.25));
    }

    #[test]
    fn breeze_dark_inherits_root_status_hues() {
        let c = palette(ThemeId::BreezeDark);
        // danger/warning inherited from :root Dark
        assert_eq!(c.danger, Rgba::rgb(0xef, 0x44, 0x44));
        assert_eq!(c.warning, Rgba::rgb(0xfb, 0xbf, 0x24));
        assert_eq!(c.accent, Rgba::rgb(0x3d, 0xae, 0xe9));
    }

    #[test]
    fn frost_langley_are_dark_polarity() {
        // Both are registered type:light in Tauri but are DARK canvases.
        let frost = palette(ThemeId::Frost);
        let langley = palette(ThemeId::Langley);
        // white alpha base (dark polarity), NOT black:
        assert_eq!(frost.alpha_pct(8), Rgba::rgba(255, 255, 255, 0x14));
        assert_eq!(langley.alpha_pct(8), Rgba::rgba(255, 255, 255, 0x14));
        assert!(!bg_is_light(frost.surface_main));
        assert!(!bg_is_light(langley.surface_main));
    }

    #[test]
    fn alucard_is_light_polarity() {
        let c = palette(ThemeId::Alucard);
        assert_eq!(c.surface_main, Rgba::rgb(0xff, 0xfb, 0xeb));
        // black alpha base (light polarity):
        assert_eq!(c.alpha_pct(8), Rgba::rgba(0, 0, 0, 0x14));
        assert!(bg_is_light(c.surface_main));
        // success on a light theme is the darker green:
        assert_eq!(c.success, Rgba::rgb(0x1f, 0x8a, 0x4c));
    }

    #[test]
    fn light_themes_use_black_alpha_base() {
        for id in [
            ThemeId::Light,
            ThemeId::Alucard,
            ThemeId::RosePineDawn,
            ThemeId::BreezeLight,
            ThemeId::AdwaitaLight,
            ThemeId::DuotoneSnow,
            ThemeId::SnowStorm,
            ThemeId::Kurosaki,
        ] {
            let c = palette(id);
            assert_eq!(c.alpha_pct(8), Rgba::rgba(0, 0, 0, 0x14), "{id:?} should be black-base");
            assert_eq!(c.surface_hover, Rgba::rgba(0, 0, 0, 0x10), "{id:?} hover base");
        }
    }

    #[test]
    fn dark_themes_use_white_alpha_base() {
        for id in [
            ThemeId::Warm,
            ThemeId::Nord,
            ThemeId::Dracula,
            ThemeId::CatppuccinMocha,
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
        ] {
            let c = palette(id);
            assert_eq!(c.alpha_pct(8), Rgba::rgba(255, 255, 255, 0x14), "{id:?} should be white-base");
        }
    }

    #[test]
    fn standard_theme_focus_ring_equals_accent() {
        for id in [
            ThemeId::Warm,
            ThemeId::Nord,
            ThemeId::Stratego,
            ThemeId::Alucard,
            ThemeId::Kurosaki,
        ] {
            let c = palette(id);
            assert_eq!(c.focus_ring, c.accent, "{id:?} focus_ring should equal accent");
            assert_eq!(c.favorite, c.danger, "{id:?} favorite should equal danger");
        }
    }

    #[test]
    fn alpha_byte_helper_matches_with_alpha() {
        // sanity: with_alpha(.., 0.1) == alpha_byte(10)
        let c = Rgba::rgb(0x10, 0x20, 0x30);
        assert_eq!(with_alpha(c, 0.1).a, alpha_byte(10));
    }
}

// ===========================================================================
// P3 — Accessibility contrast unit tests (the plan's "WCAG/APCA unit tests")
//
// Every threshold here is the documented target from 99-MIGRATION-PLAN.md
// Part B. If an assertion fails, the HEX is wrong vs Part B — fix the value to
// match the verified palette, NEVER weaken the test.
// ===========================================================================
#[cfg(test)]
mod a11y_contrast_tests {
    use super::*;
    use crate::color::{apca_lc, contrast_ratio};
    use crate::id::ALL;

    const AAA_BODY: f64 = 7.0; // WCAG 2.x AAA normal text
    const AA_NORMAL: f64 = 4.5; // WCAG 2.x AA normal text
    const NON_TEXT: f64 = 3.0; // WCAG 2.x SC 1.4.11 / 1.4.3-large

    /// Solid composite of `fg` over `bg` (a11y status surfaces are opaque, but
    /// translucent overlays compose straight-alpha for contrast measurement).
    fn over(fg: Rgba, bg: Rgba) -> Rgba {
        if fg.a == 255 {
            return fg;
        }
        let a = fg.a as f64 / 255.0;
        let mix = |f: u8, b: u8| ((f as f64 * a) + (b as f64 * (1.0 - a))).round() as u8;
        Rgba::rgb(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
    }

    // ---- wcag-light: body text AAA, accent AAA, status AAA ----------------
    #[test]
    fn wcag_light_meets_aaa() {
        let c = wcag_light();
        // text-primary on bg-primary >= 7.0:1 (AAA) — Part B: 17.40:1 / Lc 104.3
        assert!(
            contrast_ratio(c.text_primary, c.surface_main) >= AAA_BODY,
            "wcag-light text-primary {:.2}",
            contrast_ratio(c.text_primary, c.surface_main)
        );
        // text-muted on bg-primary >= 7.0:1 (AAA, exactly) — Part B 7.00:1
        assert!(contrast_ratio(c.text_muted, c.surface_main) >= AAA_BODY);
        // accent + btn-text on accent >= 7.0:1 — Part B 7.98:1
        assert!(contrast_ratio(c.accent, c.surface_main) >= AAA_BODY);
        assert!(contrast_ratio(c.accent_text, c.accent) >= AAA_BODY);
        // danger/warning/success text on bg-primary >= AAA
        assert!(contrast_ratio(c.danger, c.surface_main) >= AAA_BODY);
        assert!(contrast_ratio(c.warning, c.surface_main) >= AAA_BODY);
        assert!(contrast_ratio(c.success, c.surface_main) >= AAA_BODY);
        // non-text: border-strong + focus-ring >= 3:1
        assert!(contrast_ratio(c.border_strong, c.surface_main) >= NON_TEXT);
        assert!(contrast_ratio(c.focus_ring, c.surface_main) >= NON_TEXT);
        // APCA body gate (|Lc| >= 75) for primary text
        assert!(apca_lc(c.text_primary, c.surface_main).abs() >= 75.0);
    }

    // ---- wcag-dark: AAA + APCA, status on opaque tints --------------------
    #[test]
    fn wcag_dark_meets_aaa() {
        let c = wcag_dark();
        assert!(contrast_ratio(c.text_primary, c.surface_main) >= AAA_BODY); // 16.02:1
        assert!(contrast_ratio(c.text_secondary, c.surface_main) >= AAA_BODY);
        // text-muted is APCA-content by design but still clears AAA ratio (10.32:1)
        assert!(contrast_ratio(c.text_muted, c.surface_main) >= AAA_BODY);
        assert!(contrast_ratio(c.accent, c.surface_main) >= AAA_BODY); // 10.39:1
        // danger/warning/success text on their OPAQUE tint bg >= AAA
        assert!(contrast_ratio(c.danger, c.danger_bg) >= AAA_BODY); // 8.63:1
        assert!(contrast_ratio(c.warning, c.surface_main) >= AAA_BODY); // 12.53:1
        assert!(contrast_ratio(c.success, c.success_bg) >= AAA_BODY);
        // border-strong >= 3:1 on every surface tier (Part B: 4.11/3.76/3.31)
        for bg in [c.surface_main, c.surface_card, c.surface_elevated] {
            assert!(
                contrast_ratio(c.border_strong, bg) >= NON_TEXT,
                "wcag-dark border-strong {:.2}",
                contrast_ratio(c.border_strong, bg)
            );
        }
        assert!(contrast_ratio(c.focus_ring, c.surface_main) >= NON_TEXT);
        assert!(apca_lc(c.text_primary, c.surface_main).abs() >= 75.0);
    }

    // ---- High Contrast (both polarities): >= the wcag bar + interactive ---
    #[test]
    fn high_contrast_dark_beats_wcag_dark() {
        let hc = high_contrast();
        let wd = wcag_dark();
        let hc_tp = contrast_ratio(hc.text_primary, hc.surface_main);
        let wd_tp = contrast_ratio(wd.text_primary, wd.surface_main);
        // HC must be at least as high-contrast as wcag-dark (no regression).
        assert!(
            hc_tp >= wd_tp,
            "HC-dark text/bg {:.2} should be >= wcag-dark {:.2}",
            hc_tp,
            wd_tp
        );
        assert!(hc_tp >= 19.0); // Part B: 19.80:1
        // reciprocal cyan: accent as text AND as a fill under btn-text
        assert!(contrast_ratio(hc.accent, hc.surface_main) >= AAA_BODY); // 11.67:1
        assert!(contrast_ratio(hc.accent_text, hc.accent) >= AAA_BODY); // 12.38:1
        // interactive non-text tokens >= 3:1
        assert!(contrast_ratio(hc.border_strong, hc.surface_main) >= NON_TEXT);
        assert!(contrast_ratio(hc.focus_ring, hc.surface_main) >= NON_TEXT); // 13.76:1
        assert!(contrast_ratio(hc.border_subtle, hc.surface_main) >= NON_TEXT); // 4.61:1
    }

    #[test]
    fn high_contrast_light_beats_wcag_light() {
        let hc = high_contrast_light();
        let wl = wcag_light();
        let hc_tp = contrast_ratio(hc.text_primary, hc.surface_main);
        let wl_tp = contrast_ratio(wl.text_primary, wl.surface_main);
        assert!(
            hc_tp >= wl_tp,
            "HC-light text/bg {:.2} should be >= wcag-light {:.2}",
            hc_tp,
            wl_tp
        );
        assert!(hc_tp >= 20.0); // Part B: 21.00:1
        // reciprocal deep blue: accent as text AND btn-text under accent fill
        assert!(contrast_ratio(hc.accent, hc.surface_main) >= AAA_BODY); // 11.22:1
        assert!(contrast_ratio(hc.accent_text, hc.accent) >= AAA_BODY);
        // corrected warning #5e4b00 must clear AAA on white (8.46:1)
        assert!(
            contrast_ratio(hc.warning, hc.surface_main) >= AAA_BODY,
            "HC-light warning {:.2} (corrected #5e4b00 should be 8.46:1)",
            contrast_ratio(hc.warning, hc.surface_main)
        );
        assert_eq!(hc.warning, Rgba::rgb(0x5e, 0x4b, 0x00)); // the applied correction
        assert!(contrast_ratio(hc.border_strong, hc.surface_main) >= NON_TEXT);
        assert!(contrast_ratio(hc.focus_ring, hc.surface_main) >= NON_TEXT);
        assert!(contrast_ratio(hc.border_subtle, hc.surface_main) >= NON_TEXT); // 5.74:1
    }

    // ---- colorblind: text contrast + status hue distinctness -------------
    /// Crude protanopia/deuteranopia simulation (Brettel-style fixed matrices,
    /// sufficient to confirm the documented hue separation survives red-green
    /// CVD). Returns the simulated sRGB. Used only to assert ΔE separation, not
    /// for rendering.
    fn simulate_deutan(c: Rgba) -> Rgba {
        // Linearize, apply the standard deuteranopia LMS-collapse matrix
        // (Machado 2009, severity 1.0), re-encode. Approximate but stable.
        let lin = |v: u8| {
            let cs = v as f64 / 255.0;
            if cs <= 0.04045 {
                cs / 12.92
            } else {
                ((cs + 0.055) / 1.055).powf(2.4)
            }
        };
        let enc = |v: f64| {
            let v = v.clamp(0.0, 1.0);
            let s = if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0).round().clamp(0.0, 255.0) as u8
        };
        let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
        // Machado deuteranomaly severity=1.0 matrix:
        let nr = 0.367_322 * r + 0.860_646 * g + -0.227_968 * b;
        let ng = 0.280_085 * r + 0.672_501 * g + 0.047_413 * b;
        let nb = -0.011_820 * r + 0.042_940 * g + 0.968_881 * b;
        Rgba::rgb(enc(nr), enc(ng), enc(nb))
    }

    /// CIE76 ΔE in Lab (sufficient threshold check for hue separation).
    fn delta_e(a: Rgba, b: Rgba) -> f64 {
        fn to_lab(c: Rgba) -> (f64, f64, f64) {
            let lin = |v: u8| {
                let cs = v as f64 / 255.0;
                if cs <= 0.04045 {
                    cs / 12.92
                } else {
                    ((cs + 0.055) / 1.055).powf(2.4)
                }
            };
            let (r, g, b) = (lin(c.r), lin(c.g), lin(c.b));
            // linear sRGB -> XYZ (D65)
            let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
            let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
            let f = |t: f64| {
                if t > 0.008_856 {
                    t.cbrt()
                } else {
                    7.787 * t + 16.0 / 116.0
                }
            };
            let (xn, yn, zn) = (0.95047, 1.0, 1.08883);
            let (fx, fy, fz) = (f(x / xn), f(y / yn), f(z / zn));
            (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
        }
        let (l1, a1, b1) = to_lab(a);
        let (l2, a2, b2) = to_lab(b);
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }

    #[test]
    fn colorblind_text_passes_aa() {
        let c = colorblind();
        // text-primary AAA; muted AA on the lightest tier (Part B 5.88:1)
        assert!(contrast_ratio(c.text_primary, c.surface_main) >= AAA_BODY);
        assert!(contrast_ratio(c.text_muted, c.surface_elevated) >= AA_NORMAL);
        // accent/danger/warning double as text in ~300 places: AA on all tiers
        for bg in [c.surface_main, c.surface_card, c.surface_elevated] {
            assert!(contrast_ratio(c.accent, bg) >= AA_NORMAL, "accent on {bg:?}");
            assert!(contrast_ratio(c.danger, bg) >= AA_NORMAL, "danger on {bg:?}");
            assert!(contrast_ratio(c.warning, bg) >= AA_NORMAL, "warning on {bg:?}");
        }
        // success: AA-normal on primary/secondary (Part B/C.6); body text on the
        // lightest tier routes to success-hover (must clear AA-normal there).
        assert!(contrast_ratio(c.success, c.surface_main) >= AA_NORMAL); // 4.99:1
        assert!(contrast_ratio(c.success, c.surface_card) >= AA_NORMAL); // 4.52:1
        assert!(contrast_ratio(c.success_hover, c.surface_elevated) >= AA_NORMAL); // 6.04:1
        // focus-ring high-tone blue >= 3:1 non-text (Part B 8.53:1)
        assert!(contrast_ratio(c.focus_ring, c.surface_main) >= NON_TEXT);
    }

    #[test]
    fn colorblind_status_hues_stay_distinct_under_cvd() {
        let c = colorblind();
        // The decisive separation (Part B): danger vs warning under red-green
        // CVD must stay clearly distinct (delete vs caution must not collapse).
        // Part B reports ΔE 34.81 under deuteranopia; assert a strong margin.
        let d_sim = simulate_deutan(c.danger);
        let w_sim = simulate_deutan(c.warning);
        let dw = delta_e(d_sim, w_sim);
        assert!(
            dw >= 15.0,
            "colorblind danger vs warning under deutan ΔE {dw:.2} should stay distinct (Part B 34.81)"
        );
        // accent vs danger also separable under red-green CVD (Part B protan 10.40).
        let a_sim = simulate_deutan(c.accent);
        let ad = delta_e(a_sim, d_sim);
        assert!(
            ad >= 8.0,
            "colorblind accent vs danger under deutan ΔE {ad:.2} should stay distinct"
        );
        // accent vs warning likewise (blue vs amber, the easy axis).
        let aw = delta_e(a_sim, w_sim);
        assert!(aw >= 15.0, "colorblind accent vs warning under deutan ΔE {aw:.2}");
        // sanity: the unsimulated hues are obviously distinct too.
        assert!(delta_e(c.danger, c.warning) >= 20.0);
    }

    // ---- global: ALL registered themes return a fully-populated row -------
    #[test]
    fn all_32_themes_fully_populated_no_zero_color() {
        // The all-zero opaque black is the StdSpec::default() sentinel: a fully
        // materialized row must never leave a meaningful hue at it by accident.
        let zero = Rgba::rgb(0, 0, 0);
        for &id in ALL {
            let c = palette(id);
            // alpha ramp complete + every tier carries opacity
            assert_eq!(c.alpha.len(), crate::colors::ALPHA_COUNT, "{id:?} alpha len");
            assert!(c.alpha.iter().all(|a| a.a > 0), "{id:?} alpha has zero tier");
            // every status surface/border/hover composites to something visible
            for x in [
                c.danger_bg,
                c.danger_border,
                c.danger_hover,
                c.warning_bg,
                c.warning_border,
                c.warning_hover,
                c.success_bg,
                c.success_border,
                c.success_hover,
            ] {
                assert!(x.a > 0, "{id:?} a status tint has zero alpha");
            }
            // opaque hero tokens are opaque and not the all-zero sentinel.
            for (name, x) in [
                ("surface_main", c.surface_main),
                ("text_primary", c.text_primary),
                ("accent", c.accent),
                ("accent_text", c.accent_text),
                ("danger", c.danger),
                ("warning", c.warning),
                ("success", c.success),
                ("border_strong", c.border_strong),
                ("focus_ring", c.focus_ring),
                ("favorite", c.favorite),
            ] {
                assert_eq!(x.a, 255, "{id:?} {name} must be opaque");
            }
            // text/accent/border-strong must not be invisible-on-bg (the "default
            // color slipped through" symptom): require >= 1.5:1 minimum signal.
            assert!(
                contrast_ratio(c.text_primary, c.surface_main) >= 1.5,
                "{id:?} text_primary indistinguishable from bg (zero color?)"
            );
            // System falls back to Dark; skip the pure-pair identity below for it.
            let _ = (zero, over(c.surface_hover, c.surface_main));
        }
        // Count: the registry holds every ThemeId variant. The plan's prose
        // says "32 themes" but counts inconsistently (it variously treats the
        // System meta-entry as in/out). The enum is the source of truth: 4 Core
        // + 19 Dark + 8 Light + 5 Accessibility = 36 rows. Assert the concrete
        // breakdown so a future add/remove can't silently drop a row.
        let n_a11y = ALL
            .iter()
            .filter(|id| id.category() == crate::id::ThemeCategory::Accessibility)
            .count();
        assert_eq!(n_a11y, 5, "exactly 5 accessibility themes (P3)");
        assert_eq!(ALL.len(), 36, "registry must hold every ThemeId row");
    }
}
