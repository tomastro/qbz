//! Custom-theme editor controller — the Qt port of `crates/qbz/src/
//! custom_theme.rs`, minus the Slint-property plumbing.
//!
//! The user-editable base is `qbz_theme::CustomThemeBase` (11 opaque colour
//! tokens + `is_dark`); the full 55-value `ThemeColors` contract is DERIVED
//! from it by the shared `qbz_theme::theme_from_base`. Neither the maths nor
//! the schema is re-implemented here — this module only owns the in-memory
//! base, the persistence and the two bridge documents.
//!
//! # The file is SHARED with the shipping Slint build
//!
//! `<data_dir>/qbz/custom_theme.json` is the SAME document the Slint
//! custom-theme editor writes, and both sides serialize the SAME struct
//! through the SAME serde impl, so byte-compatibility is satisfied by
//! construction — there is no second file and no second format. Two
//! consequences, both deliberate:
//!
//!   * Whole-document read-modify-write with no other owner: last writer wins
//!     for the WHOLE theme, and there is no cross-key clobber hazard (unlike
//!     `ui_prefs.json`, which is why `settings_qt` documents an additive-patch
//!     discipline for that file). The other process keeps its stale derived
//!     palette until it re-reads: on this side the memo below is dropped when
//!     the theme row enters "custom" (`crate::theme_set`), which is the only
//!     moment a foreign write can matter.
//!   * Qt CREATING this file for the first time (the first-selection seed in
//!     `crate::theme_set`) flips `custom_theme::exists()` for the shipping
//!     Slint binary on that profile, so ITS first-selection branch takes
//!     `seed_state + apply_startup` from our file instead of seeding from what
//!     it is showing. Known, accepted: one shared file is the requirement.
//!
//! # Two deliberate improvements on the reference
//!
//!   * **Atomic publish.** The reference persists with a truncating
//!     `std::fs::write` (`custom_theme.rs:144`), which can hand a concurrent
//!     reader — including the shipping Slint build — a half document that
//!     parses as garbage and degrades to OLED. We write a sibling temp file
//!     and `rename(2)` it on, the same mechanic `settings_qt::
//!     write_json_object_atomic` uses for `ui_prefs.json`. It is NOT that
//!     function: that one takes a `serde_json::Map`, which is a `BTreeMap`
//!     here (no `preserve_order` feature), so it would reorder the keys
//!     alphabetically. Serializing the STRUCT keeps the reference's field
//!     order, i.e. byte-identical output.
//!   * **Debounced write.** The reference persists inside `apply_live`, i.e.
//!     potentially on every drag frame. Re-derive + republish stays live (pure
//!     maths, no I/O); only the WRITE is debounced to ~250 ms idle, and the
//!     picker's drag-end calls `flush()` so a settled drag lands immediately.
//!     Discrete actions (polarity toggle, seed) write straight away.
//!
//! # Two encodings, and they must not mix
//!
//!   * `#rrggbb` — 6 digits, lowercase — on the EDIT and PERSIST path. That is
//!     what `Rgba::to_hex` writes and what the file contains.
//!   * `#aarrggbb` — on the RENDER/DISPLAY path (`custom_theme_json`), like
//!     every other Qt theme token.
//!
//! Copying `theme_qt::argb()` into the edit path is the bug this note exists
//! to prevent.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use cxx_qt_lib::QString;
use qbz_theme::{CustomThemeBase, Rgba, ThemeColors};

/// The stable token keys — kebab-case, the SAME vocabulary the Slint editor
/// rows use (`custom_theme.rs:56-68`). The on-disk JSON keys are snake_case
/// (they are the Rust field names); these are the UI/bridge keys. Order is the
/// editor's reading order (SURFACES, TEXT, ACCENT, STATUS, OTHER).
pub(crate) const TOKEN_KEYS: [&str; 11] = [
    "surface-main",
    "surface-card",
    "surface-elevated",
    "text-primary",
    "text-secondary",
    "accent",
    "danger",
    "warning",
    "success",
    "border",
    "favorite",
];

/// `<data_dir>/qbz/custom_theme.json` — sibling of `ui_prefs.json`, and the
/// one spelling of this path in the crate (`theme_qt` reads it through here).
pub(crate) fn custom_theme_path() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("qbz").join("custom_theme.json"))
}

/// The in-memory base — the Qt analogue of "the swatch properties are the
/// source of truth while the editor is open" (`custom_theme.rs:32-34`). Filled
/// lazily from disk on first use so a per-drag edit never reconstructs the
/// base by reading the file back.
static BASE: Mutex<Option<CustomThemeBase>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<CustomThemeBase>> {
    BASE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read + parse the file. `None` = absent, unreadable, or corrupt (logged);
/// the caller degrades to the OLED default, never errors — a hand-edited file
/// can never take the app down (`custom_theme.rs:116-127`).
fn read_file() -> Option<CustomThemeBase> {
    let path = custom_theme_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<CustomThemeBase>(&text) {
        Ok(base) => Some(base),
        Err(e) => {
            log::warn!("[qbz-qt] custom_theme.json parse failed, using default: {e}");
            None
        }
    }
}

/// The current editable base. Non-persisting: a missing file yields the OLED
/// default WITHOUT creating anything (the file is only born when the user
/// actually selects or edits the Custom theme).
pub(crate) fn load() -> CustomThemeBase {
    let mut guard = lock();
    if guard.is_none() {
        *guard = Some(read_file().unwrap_or_else(CustomThemeBase::default_oled));
    }
    (*guard)
        .clone()
        .unwrap_or_else(CustomThemeBase::default_oled)
}

/// Drop the memo so the next `load()` goes back to disk.
///
/// The cache is what keeps a per-drag edit from reading the file back, but it
/// is filled at bridge construction for EVERY user, whatever their theme — so
/// a process that booted without the file would keep serving the OLED default
/// it cached even after the file appeared. That is not hypothetical: this file
/// is SHARED with the shipping Slint build, which can write it while this
/// process is running, and `exists()` is a live disk check, so the
/// first-selection seed would be skipped in favour of a cache that never saw
/// the user's theme. Called when the theme row enters "custom" — a user
/// action, never on the drag path.
pub(crate) fn invalidate() {
    *lock() = None;
}

/// True when a persisted custom base exists on disk — the first-selection
/// check (`custom_theme.rs:155-157`).
pub(crate) fn exists() -> bool {
    custom_theme_path().map(|p| p.exists()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Persistence — atomic temp + rename, byte-identical content to the reference
// ---------------------------------------------------------------------------

fn save(base: &CustomThemeBase) {
    use std::io::Write as _;

    let Some(path) = custom_theme_path() else {
        log::warn!("[qbz-qt] custom_theme.json: data dir unavailable, not saving");
        return;
    };
    // `to_string_pretty` on the STRUCT: same serializer, same field order and
    // same 2-space indent the shipping build writes.
    let Ok(text) = serde_json::to_string_pretty(base) else {
        log::warn!("[qbz-qt] custom_theme.json: serialize failed — not writing");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("[qbz-qt] custom_theme.json: parent directory unavailable ({e})");
            return;
        }
    }
    // `<path>.<pid>.tmp`, built off the FULL path so it lands in the same
    // directory — rename is only atomic within one filesystem.
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = std::path::PathBuf::from(tmp);
    let written = std::fs::File::create(&tmp).and_then(|mut f| {
        f.write_all(text.as_bytes())?;
        f.sync_all()
    });
    if let Err(e) = written {
        log::warn!("[qbz-qt] custom_theme.json: temp write failed: {e}");
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("[qbz-qt] custom_theme.json: atomic rename failed: {e}");
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Idle window before a live edit is persisted. A drag emits one edit per
/// mouse-move frame; the picker also calls [`flush`] on release, so this only
/// covers the "stopped moving but still holding" case and keyboard hex edits.
const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Bumped by every live edit. The debounced writer only publishes when the
/// value it captured is still the newest — the newest edit wins and the
/// intermediate frames never touch the disk.
static WRITE_SEQ: AtomicU64 = AtomicU64::new(0);
/// Set by a live edit, cleared by an actual write, so [`flush`] on a drag that
/// moved nothing does not rewrite an unchanged file.
static DIRTY: AtomicBool = AtomicBool::new(false);

fn save_now() {
    DIRTY.store(false, Ordering::SeqCst);
    let base = load();
    save(&base);
}

/// Mark the in-memory base dirty and schedule the debounced write.
fn schedule_save() {
    DIRTY.store(true, Ordering::SeqCst);
    let seq = WRITE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    crate::spawn(async move {
        tokio::time::sleep(WRITE_DEBOUNCE).await;
        if WRITE_SEQ.load(Ordering::SeqCst) == seq && DIRTY.load(Ordering::SeqCst) {
            save_now();
        }
    });
}

/// Drag-end / commit flush: persist NOW if anything is pending. Bumping the
/// sequence first makes any in-flight debounce task drop its own write.
pub(crate) fn flush() {
    WRITE_SEQ.fetch_add(1, Ordering::SeqCst);
    if DIRTY.load(Ordering::SeqCst) {
        save_now();
    }
}

// ---------------------------------------------------------------------------
// Token access by key
// ---------------------------------------------------------------------------

fn field<'a>(base: &'a CustomThemeBase, key: &str) -> Option<&'a String> {
    Some(match key {
        "surface-main" => &base.surface_main,
        "surface-card" => &base.surface_card,
        "surface-elevated" => &base.surface_elevated,
        "text-primary" => &base.text_primary,
        "text-secondary" => &base.text_secondary,
        "accent" => &base.accent,
        "danger" => &base.danger,
        "warning" => &base.warning,
        "success" => &base.success,
        "border" => &base.border,
        "favorite" => &base.favorite,
        _ => return None,
    })
}

fn field_mut<'a>(base: &'a mut CustomThemeBase, key: &str) -> Option<&'a mut String> {
    Some(match key {
        "surface-main" => &mut base.surface_main,
        "surface-card" => &mut base.surface_card,
        "surface-elevated" => &mut base.surface_elevated,
        "text-primary" => &mut base.text_primary,
        "text-secondary" => &mut base.text_secondary,
        "accent" => &mut base.accent,
        "danger" => &mut base.danger,
        "warning" => &mut base.warning,
        "success" => &mut base.success,
        "border" => &mut base.border,
        "favorite" => &mut base.favorite,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// The bridge documents
// ---------------------------------------------------------------------------

/// DISPLAY encoding for a stored base token: `#aarrggbb`, what QbzTheme.qml
/// and every other Qt theme token use. Base tokens are opaque by contract, so
/// the alpha byte is always `ff`; a malformed stored value shows as opaque
/// black here while the derivation applies its own per-token fallback.
fn display_hex(hex6: &str) -> String {
    let c = Rgba::from_hex(hex6).unwrap_or(Rgba::rgb(0, 0, 0));
    format!("#ff{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// `{"isDark":bool,"tokens":{"<kebab-key>":"#aarrggbb"}}` — the swatch grid's
/// whole state. A MAP and not eleven named fields so QML indexes it by the
/// same token key the cells carry, instead of an eleven-arm ternary chain.
fn state_json_of(base: &CustomThemeBase) -> String {
    let mut tokens = serde_json::Map::new();
    for key in TOKEN_KEYS {
        let hex6 = field(base, key).map(String::as_str).unwrap_or("#000000");
        tokens.insert(
            key.to_string(),
            serde_json::Value::String(display_hex(hex6)),
        );
    }
    let mut doc = serde_json::Map::new();
    doc.insert("isDark".into(), serde_json::Value::Bool(base.is_dark));
    doc.insert("tokens".into(), serde_json::Value::Object(tokens));
    serde_json::to_string(&doc).unwrap_or_else(|_| "{}".into())
}

/// The editor state for the CURRENT base (used to seed the bridge property at
/// construction — a post-boot push would be a frame late, same reason
/// `theme_json` is seeded there).
pub(crate) fn state_json() -> String {
    state_json_of(&load())
}

fn publish_state_of(base: &CustomThemeBase) {
    let json = state_json_of(base);
    crate::shell_bridge::ui(move |mut b| {
        b.as_mut()
            .set_custom_theme_json(QString::from(json.as_str()));
    });
}

/// Republish the editor state from whatever the base currently is.
pub(crate) fn publish_state() {
    publish_state_of(&load());
}

/// Derive + push the palette live. Pure maths, no I/O — this is the Qt
/// `theme::push_colors` half of the reference's `apply_live`
/// (`custom_theme.rs:96-100`); the persistence half is debounced separately.
///
/// It republishes from the IN-MEMORY base rather than going through
/// `theme_qt::publish_theme()`, which re-resolves the slug off disk and would
/// therefore show the last PERSISTED colours during the debounce window.
fn apply_live(base: &CustomThemeBase) {
    crate::theme_qt::publish_colors(&qbz_theme::theme_from_base(base));
}

// ---------------------------------------------------------------------------
// Editor actions (the bridge invokables)
// ---------------------------------------------------------------------------

/// Set one base token from a `#rrggbb` (or `#rrggbbaa`, alpha dropped) string
/// — both the live drag path and the HEX-field commit. Malformed input and
/// unknown keys are IGNORED, 1:1 with `set_token_hex` (`custom_theme.rs:
/// 222-227`): a half-typed hex must not repaint the app.
pub(crate) fn set_token(key: &str, hex: &str) {
    let Some(c) = Rgba::from_hex(hex) else {
        log::debug!("[qbz-qt] custom theme: ignoring malformed hex '{hex}'");
        return;
    };
    // Alpha is dropped: base tokens are opaque by contract, and the file is
    // 6-digit. `to_hex` IS the persist encoding — never `theme_qt::argb`.
    let hex6 = Rgba::rgb(c.r, c.g, c.b).to_hex();
    let base = {
        let mut guard = lock();
        if guard.is_none() {
            *guard = Some(read_file().unwrap_or_else(CustomThemeBase::default_oled));
        }
        let Some(base) = guard.as_mut() else {
            return;
        };
        let Some(slot) = field_mut(base, key) else {
            log::debug!("[qbz-qt] custom theme: unknown token key '{key}'");
            return;
        };
        if *slot == hex6 {
            return;
        }
        *slot = hex6;
        base.clone()
    };
    apply_live(&base);
    publish_state_of(&base);
    schedule_save();
}

/// Flip the polarity. The base token colours are unchanged; only `is_dark` and
/// everything derived from it (alpha ramp, translucent edges, shade
/// directions) flip. A discrete click, so it persists immediately.
pub(crate) fn toggle_dark(is_dark: bool) {
    let base = {
        let mut guard = lock();
        if guard.is_none() {
            *guard = Some(read_file().unwrap_or_else(CustomThemeBase::default_oled));
        }
        let Some(base) = guard.as_mut() else {
            return;
        };
        if base.is_dark == is_dark {
            return;
        }
        base.is_dark = is_dark;
        base.clone()
    };
    apply_live(&base);
    publish_state_of(&base);
    DIRTY.store(true, Ordering::SeqCst);
    flush();
}

/// "Start from current theme": reduce an applied palette to the editable base
/// and PERSIST it. Does not publish anything — the caller decides, because the
/// two callers want different follow-ups (the button applies live; the
/// first-selection seed lets `theme_set`'s own publish do it once).
///
/// `prev` is passed IN rather than read here, and that is load-bearing for the
/// first-selection seed: `theme_set` persists the new slug BEFORE it could
/// call us, and `theme_qt::current_slug()` re-reads the pref FROM DISK — so a
/// self-service read would resolve `"custom"`, find no file, and snapshot OLED
/// black, which is exactly the bug the seed exists to prevent. Slint gets away
/// with reading live state because it has an in-memory palette global; the Qt
/// bridge carries only `#aarrggbb` text.
///
/// Two subtleties ported verbatim from `custom_theme.rs:244-276`: `is_dark` is
/// INFERRED from the surface luminance (not read from a pref), and `border`
/// prefers `border_subtle` only when it is opaque — `base_from_theme`
/// implements that second rule, so it is reused rather than re-hand-rolled.
pub(crate) fn seed_from_current(prev: &ThemeColors) -> CustomThemeBase {
    let sm = prev.surface_main;
    let is_dark = qbz_theme::relative_luminance(Rgba::rgb(sm.r, sm.g, sm.b)) < 0.5;
    let base = qbz_theme::base_from_theme(prev, is_dark);
    *lock() = Some(base.clone());
    // A discrete action: no debounce, and no stale pending write left behind.
    DIRTY.store(false, Ordering::SeqCst);
    WRITE_SEQ.fetch_add(1, Ordering::SeqCst);
    save(&base);
    base
}

/// The "Use current colors" button: snapshot whatever palette is applied right
/// now (which, since the editor is only reachable under the Custom theme, is
/// the custom palette itself — 1:1 with the reference, whose seed is
/// idempotent by `custom.rs`'s own round-trip test).
pub(crate) fn seed_from_applied() {
    let colors = crate::theme_qt::colors_for_slug(&crate::theme_qt::current_slug());
    let base = seed_from_current(&colors);
    apply_live(&base);
    publish_state_of(&base);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CustomThemeBase {
        CustomThemeBase::default_oled()
    }

    #[test]
    fn token_keys_all_resolve_to_a_field() {
        let base = sample();
        for key in TOKEN_KEYS {
            assert!(field(&base, key).is_some(), "no field for '{key}'");
        }
        assert!(field(&base, "nope").is_none());
        assert!(field(&base, "is-dark").is_none());
    }

    #[test]
    fn token_keys_cover_every_editable_field() {
        // Mutating through every key must touch a DISTINCT field: 11 keys,
        // 11 changed values. Catches a copy-paste arm pointing at the wrong
        // token (which would silently edit the wrong swatch).
        let mut base = sample();
        for (i, key) in TOKEN_KEYS.iter().enumerate() {
            *field_mut(&mut base, key).unwrap() = format!("#0000{:02x}", i as u8);
        }
        let mut seen: Vec<&String> = TOKEN_KEYS
            .iter()
            .map(|k| field(&base, k).unwrap())
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), TOKEN_KEYS.len());
    }

    #[test]
    fn display_hex_is_argb_and_edit_hex_is_rgb() {
        // The whole two-encoding discipline in one assertion.
        assert_eq!(display_hex("#4285f4"), "#ff4285f4");
        assert_eq!(Rgba::rgb(0x42, 0x85, 0xf4).to_hex(), "#4285f4");
        // Alpha in, alpha dropped on the persist path.
        let c = Rgba::from_hex("#4285f480").unwrap();
        assert_eq!(Rgba::rgb(c.r, c.g, c.b).to_hex(), "#4285f4");
        // Malformed stored token displays as opaque black, never panics.
        assert_eq!(display_hex("not-a-color"), "#ff000000");
    }

    #[test]
    fn state_json_carries_all_eleven_tokens_plus_polarity() {
        let doc: serde_json::Value = serde_json::from_str(&state_json_of(&sample())).unwrap();
        assert_eq!(doc["isDark"], serde_json::Value::Bool(true));
        let tokens = doc["tokens"].as_object().unwrap();
        assert_eq!(tokens.len(), TOKEN_KEYS.len());
        for key in TOKEN_KEYS {
            let v = tokens[key].as_str().unwrap();
            assert_eq!(v.len(), 9, "{key} is not #aarrggbb");
            assert!(v.starts_with("#ff"), "{key} base tokens must be opaque");
        }
        assert_eq!(tokens["surface-main"].as_str().unwrap(), "#ff000000");
        assert_eq!(tokens["accent"].as_str().unwrap(), "#ff4285f4");
    }

    #[test]
    fn persisted_shape_matches_the_reference() {
        // The on-disk document: flat, snake_case, 12 keys, 6-digit colours,
        // no version field. The shipping Slint build parses THIS.
        let text = serde_json::to_string_pretty(&sample()).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        let obj = doc.as_object().unwrap();
        assert_eq!(obj.len(), 12);
        assert!(obj["is_dark"].is_boolean());
        for key in [
            "surface_main",
            "surface_card",
            "surface_elevated",
            "text_primary",
            "text_secondary",
            "accent",
            "danger",
            "warning",
            "success",
            "border",
            "favorite",
        ] {
            let v = obj[key].as_str().unwrap();
            assert_eq!(v.len(), 7, "{key} must be #rrggbb");
            assert_eq!(v, v.to_lowercase());
        }
        assert!(obj.get("version").is_none());
        // And it round-trips back through the shared struct.
        let back: CustomThemeBase = serde_json::from_str(&text).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn seed_infers_polarity_from_surface_luminance() {
        // The rule the reference applies at custom_theme.rs:247-251.
        let dark = qbz_theme::palette(qbz_theme::ThemeId::Oled);
        assert!(qbz_theme::relative_luminance(dark.surface_main) < 0.5);
        let light = qbz_theme::palette(qbz_theme::ThemeId::Light);
        assert!(qbz_theme::relative_luminance(light.surface_main) >= 0.5);
    }

    #[test]
    fn seed_border_prefers_the_opaque_edge() {
        // base_from_theme owns the rule; assert it holds for a palette whose
        // border_subtle is translucent (the legacy P1 hairline).
        let mut colors = qbz_theme::palette(qbz_theme::ThemeId::Oled);
        colors.border_subtle = Rgba::rgba(0xff, 0xff, 0xff, 0x14);
        colors.border_strong = Rgba::rgb(0x3a, 0x3a, 0x3a);
        let base = qbz_theme::base_from_theme(&colors, true);
        assert_eq!(base.border, "#3a3a3a");
    }
}
