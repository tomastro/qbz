//! Theme controller (phase 19) — the Slint `crates/qbz/src/theme.rs` port:
//! the persisted `ui_prefs.theme` slug resolves through the SAME `qbz-theme`
//! registry (`ThemeId`/`palette()`), and the materialized `ThemeColors` are
//! serialized to ONE JSON document that QbzTheme.qml binds to — the Qt
//! equivalent of `theme::push_colors` (live switch, no restart).
//!
//! Synthetic slugs (theme.rs): "auto" (AutoSource::System regeneration —
//! falls back to OLED when headless generation fails) and "custom"
//! (`<data_dir>/qbz/custom_theme.json`, the SAME file the Slint custom-theme
//! editor writes — the editor and the write path live in `custom_theme_qt`,
//! which also owns the path and the in-memory base this module reads). The
//! "system" registry theme resolves via the OS palette in Slint; Qt currently
//! maps that one slug to the Dark palette, a documented parity gap.

use cxx_qt_lib::QString;
use qbz_theme::{Rgba, ThemeColors, ThemeId};
use serde::Serialize;

// ---------------------------------------------------------------------------
// ui_prefs.json keys (theme + theme_filter).
//
// Writes go through `settings_qt::save_pref` — the ONE atomic read-modify-write
// of this shared file (see the discipline block in settings_qt.rs). This module
// used to carry its own copy: a truncating `std::fs::write` plus a
// `json!({})` fallback, on the same document the SHIPPING Slint build has open.
// Switching theme while that build ran could hand its `ui_prefs::load()` an
// empty file, and its next save would flatten npb_mode, streaming_quality,
// cast_quality_caps, sidebar_state, renderer and the window geometry back to
// defaults — the whole profile, for a theme click. Reads stay local (a read
// cannot corrupt anything) but share the one spelling of the path.
// ---------------------------------------------------------------------------

fn read_pref(key: &str) -> Option<serde_json::Value> {
    let path = crate::settings_qt::prefs_path()?;
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get(key).cloned())
}

/// The persisted theme slug (ui_prefs.rs default: "oled").
pub fn current_slug() -> String {
    read_pref("theme")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "oled".to_string())
}

/// The dropdown filter (theme.rs: 0 All / 1 Dark / 2 Light; default 0).
pub fn theme_filter() -> i32 {
    read_pref("theme_filter")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32
}

pub fn set_theme_filter(index: i32) {
    crate::settings_qt::save_pref("theme_filter", serde_json::json!(index.clamp(0, 2)));
}

/// Persist a new slug (additive); the caller republishes `themeJson`.
pub fn set_theme(slug: &str) {
    crate::settings_qt::save_pref("theme", serde_json::Value::String(slug.to_string()));
    log::info!("[qbz-qt] theme -> {slug}");
}

// ---------------------------------------------------------------------------
// Slug -> ThemeColors (theme.rs apply_theme / auto / custom startup dispatch)
// ---------------------------------------------------------------------------

/// The user-authored theme: the 11 editable base tokens expanded through the
/// SHARED derivation. The base (and the file path, and the write path) belong
/// to `custom_theme_qt` — reading it back off disk here would show the last
/// PERSISTED colours during the editor's debounce window instead of the live
/// ones.
fn custom_colors() -> ThemeColors {
    qbz_theme::theme_from_base(&crate::custom_theme_qt::load())
}

/// Build the generator's source from the persisted pref — 1:1 with the
/// reference's `auto_theme::source_from_prefs` (`crates/qbz/src/auto_theme.rs:
/// 20-26`), including the "anything unknown means System" fallback.
///
/// This port used to hard-code `AutoSource::System`, which left the
/// Appearance "Source" row (System Colors / Wallpaper Sync / Custom Image)
/// selectable, persisted, and completely inert: picking Wallpaper Sync
/// regenerated the SAME system palette.
fn auto_source() -> qbz_theme::AutoSource {
    match crate::settings_qt::pref_str("auto_theme_source", "system").as_str() {
        "wallpaper" => qbz_theme::AutoSource::Wallpaper,
        "image" => {
            qbz_theme::AutoSource::Image(crate::settings_qt::pref_str("auto_theme_image_path", ""))
        }
        _ => qbz_theme::AutoSource::System,
    }
}

fn auto_colors() -> ThemeColors {
    match qbz_theme::generate_auto_theme(&auto_source()) {
        Ok(c) => c,
        Err(e) => {
            // Headless / no DE portal, or a picked image that has since been
            // moved: keep the row selectable and fall back to the default
            // dark tokens.
            log::warn!("[qbz-qt] auto theme generation failed ({e}); falling back to OLED");
            qbz_theme::palette(qbz_theme::default_theme_id())
        }
    }
}

/// Resolve a slug to its materialized colors (unknown -> OLED, theme.rs
/// `id_for_slug` fallback).
pub fn colors_for_slug(slug: &str) -> ThemeColors {
    match slug {
        "auto" => auto_colors(),
        "custom" => custom_colors(),
        // "system" reads the OS palette in Slint; Qt currently maps it to the
        // Dark registry palette.
        "system" => qbz_theme::palette(ThemeId::Dark),
        other => ThemeId::from_slug(other)
            .map(qbz_theme::palette)
            .unwrap_or_else(|| qbz_theme::palette(qbz_theme::default_theme_id())),
    }
}

// ---------------------------------------------------------------------------
// ThemeColors -> QML token JSON (theme::push_colors equivalent)
// ---------------------------------------------------------------------------

/// QML color literal: "#aarrggbb" (Qt ARGB — the notation QbzTheme.qml
/// already uses).
fn argb(c: Rgba) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.a, c.r, c.g, c.b)
}

fn with_alpha(c: Rgba, a: u8) -> String {
    argb(Rgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    })
}

/// The alpha-ramp percents (colors.rs:12-17) — indices into the 24 tiers.
const ALPHA_PCTS: [u32; 24] = [
    4, 5, 6, 8, 10, 12, 15, 18, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95,
];

#[derive(Serialize)]
struct ThemeTokens {
    // 31 named colors (ThemeColors contract).
    #[serde(rename = "surfaceMain")]
    surface_main: String,
    #[serde(rename = "surfaceCard")]
    surface_card: String,
    #[serde(rename = "surfaceElevated")]
    surface_elevated: String,
    #[serde(rename = "surfaceHover")]
    surface_hover: String,
    #[serde(rename = "bgHover")]
    bg_hover: String,
    #[serde(rename = "textPrimary")]
    text_primary: String,
    #[serde(rename = "textSecondary")]
    text_secondary: String,
    #[serde(rename = "textMuted")]
    text_muted: String,
    #[serde(rename = "textDisabled")]
    text_disabled: String,
    accent: String,
    #[serde(rename = "accentHover")]
    accent_hover: String,
    #[serde(rename = "accentPressed")]
    accent_pressed: String,
    #[serde(rename = "accentText")]
    accent_text: String,
    danger: String,
    #[serde(rename = "dangerBg")]
    danger_bg: String,
    #[serde(rename = "dangerBorder")]
    danger_border: String,
    #[serde(rename = "dangerHover")]
    danger_hover: String,
    warning: String,
    #[serde(rename = "warningBg")]
    warning_bg: String,
    #[serde(rename = "warningBorder")]
    warning_border: String,
    #[serde(rename = "warningHover")]
    warning_hover: String,
    success: String,
    #[serde(rename = "successBg")]
    success_bg: String,
    #[serde(rename = "successBorder")]
    success_border: String,
    #[serde(rename = "successHover")]
    success_hover: String,
    #[serde(rename = "borderSubtle")]
    border_subtle: String,
    #[serde(rename = "borderMuted")]
    border_muted: String,
    #[serde(rename = "borderStrong")]
    border_strong: String,
    #[serde(rename = "focusRing")]
    focus_ring: String,
    favorite: String,
    #[serde(rename = "cardShadow")]
    card_shadow: String,
    // 24 alpha tiers (alpha4..alpha95, white-based dark / black-based light).
    alpha: Vec<String>,
    // Ambient-layer derivations (phase-14 tokens, now theme-derived):
    // chrome surface-card @50%, frosted panel surface-main @22%, thin bars
    // surface-main @30%, hairline = alpha tier 10%.
    //
    // The two @50% siblings below carry the SAME `app-background-surface-alpha`
    // (0.5) the chrome does — the Slint applies it to three different tokens
    // depending on the surface's tier: surface-card for chrome
    // (Sidebar/HeaderBar/PlayerBar/the content frame), surface-elevated for
    // controls sitting ON a panel (ToggleButton/QbzSelect/SegmentedTabBar/
    // CircleAction/the header search field), and surface-main for the Large
    // dock's art well (SidebarNowPlayingDock.slint:193).
    #[serde(rename = "surfaceCardA50")]
    surface_card_a50: String,
    #[serde(rename = "surfaceElevatedA50")]
    surface_elevated_a50: String,
    #[serde(rename = "surfaceMainA50")]
    surface_main_a50: String,
    #[serde(rename = "surfaceMainA22")]
    surface_main_a22: String,
    #[serde(rename = "surfaceMainA30")]
    surface_main_a30: String,
    #[serde(rename = "frostBorder")]
    frost_border: String,
    // Flags (Theme.is-dark).
    #[serde(rename = "isDark")]
    is_dark: bool,
}

fn tokens_for(colors: &ThemeColors) -> ThemeTokens {
    let is_dark = qbz_theme::relative_luminance(colors.surface_main) < 0.5;
    ThemeTokens {
        surface_main: argb(colors.surface_main),
        surface_card: argb(colors.surface_card),
        surface_elevated: argb(colors.surface_elevated),
        surface_hover: argb(colors.surface_hover),
        bg_hover: argb(colors.bg_hover),
        text_primary: argb(colors.text_primary),
        text_secondary: argb(colors.text_secondary),
        text_muted: argb(colors.text_muted),
        text_disabled: argb(colors.text_disabled),
        accent: argb(colors.accent),
        accent_hover: argb(colors.accent_hover),
        accent_pressed: argb(colors.accent_pressed),
        accent_text: argb(colors.accent_text),
        danger: argb(colors.danger),
        danger_bg: argb(colors.danger_bg),
        danger_border: argb(colors.danger_border),
        danger_hover: argb(colors.danger_hover),
        warning: argb(colors.warning),
        warning_bg: argb(colors.warning_bg),
        warning_border: argb(colors.warning_border),
        warning_hover: argb(colors.warning_hover),
        success: argb(colors.success),
        success_bg: argb(colors.success_bg),
        success_border: argb(colors.success_border),
        success_hover: argb(colors.success_hover),
        border_subtle: argb(colors.border_subtle),
        border_muted: argb(colors.border_muted),
        border_strong: argb(colors.border_strong),
        focus_ring: argb(colors.focus_ring),
        favorite: argb(colors.favorite),
        card_shadow: argb(colors.card_shadow),
        alpha: colors.alpha.iter().map(|c| argb(*c)).collect(),
        surface_card_a50: with_alpha(colors.surface_card, 0x80),
        surface_elevated_a50: with_alpha(colors.surface_elevated, 0x80),
        surface_main_a50: with_alpha(colors.surface_main, 0x80),
        surface_main_a22: with_alpha(colors.surface_main, 0x38),
        surface_main_a30: with_alpha(colors.surface_main, 0x4d),
        frost_border: argb(colors.alpha[ALPHA_PCTS.iter().position(|p| *p == 10).unwrap_or(4)]),
        is_dark,
    }
}

/// The current theme as the QML token document.
pub fn theme_json() -> String {
    let colors = colors_for_slug(&current_slug());
    serde_json::to_string(&tokens_for(&colors)).unwrap_or_else(|_| "{}".into())
}

/// Push an ALREADY-RESOLVED palette — the custom-theme editor's live preview
/// (`custom_theme_qt::apply_live`). Deliberately does NOT touch `theme_slug`
/// and deliberately does NOT re-resolve the slug: during a colour drag the
/// authority is the in-memory base, not the (debounced) file on disk.
pub(crate) fn publish_colors(colors: &ThemeColors) {
    let json = serde_json::to_string(&tokens_for(colors)).unwrap_or_else(|_| "{}".into());
    crate::shell_bridge::ui(move |mut b| {
        b.as_mut().set_theme_json(QString::from(json.as_str()));
    });
}

/// Re-resolve + republish after a slug change (the push_colors moment).
pub fn publish_theme() {
    let json = theme_json();
    let slug = current_slug();
    crate::shell_bridge::ui(move |mut b| {
        b.as_mut().set_theme_json(QString::from(json.as_str()));
        b.as_mut().set_theme_slug(QString::from(slug.as_str()));
    });
}

// ---------------------------------------------------------------------------
// The dropdown catalog (theme.rs dropdown_labels + the 2 synthetic rows)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ThemeEntry {
    label: String,
    slug: String,
    #[serde(rename = "isLight")]
    is_light: bool,
}

/// All 36 registry themes + "Auto (dynamic)" + "Custom" (theme.rs:128-142,
/// 217-227 — the synthetics only show under the All filter; QML filters).
pub fn theme_list_json() -> String {
    let mut entries: Vec<ThemeEntry> = qbz_theme::theme_list()
        .into_iter()
        .map(|e| ThemeEntry {
            label: e.display_name.to_string(),
            slug: e.slug.to_string(),
            is_light: e.is_light,
        })
        .collect();
    entries.push(ThemeEntry {
        label: "Auto (dynamic)".to_string(),
        slug: "auto".to_string(),
        is_light: false,
    });
    entries.push(ThemeEntry {
        label: "Custom".to_string(),
        slug: "custom".to_string(),
        is_light: false,
    });
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into())
}
