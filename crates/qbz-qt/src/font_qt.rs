//! Script-specific application font fallbacks.
//!
//! The UI's "System" choice remains Qt's real system font for covered glyphs.
//! This module only gives Qt deterministic bundled faces when that font (or a
//! selected UI/lyrics font) lacks Japanese or Devanagari, avoiding arbitrary
//! and sometimes visually incompatible host fallbacks.

unsafe extern "C" {
    fn qbz_register_devanagari_fallback() -> bool;
    fn qbz_register_japanese_fallback() -> bool;
}

pub(crate) fn register_script_fallbacks() {
    // SAFETY: these C++ functions have no arguments, are called once on the
    // GUI thread after QGuiApplication exists, and only mutate QFontDatabase's
    // documented process-global application-font registry.
    if unsafe { qbz_register_japanese_fallback() } {
        log::info!("[qbz-qt] font fallback -> LINE Seed JP (Han/Hiragana/Katakana)");
    } else {
        log::warn!("[qbz-qt] bundled LINE Seed JP fallback failed to load");
    }
    if unsafe { qbz_register_devanagari_fallback() } {
        log::info!("[qbz-qt] font fallback -> Noto Sans Devanagari");
    } else {
        log::warn!("[qbz-qt] bundled Noto Sans Devanagari fallback failed to load");
    }
}
