//! QbzImmersive — immersive-mode domain bridge (viz_bridge.rs exemplar:
//! OnceLock<CxxQtThread>, `ui()` no-op pre-boot, `boot()` invokable, `impl
//! Default` for construction-seeded values).
//!
//! Port of the Slint `ImmersiveState` global per the immersive-port contract
//! (`qbz-nix-docs/qt-frontend/2026-08-02-immersive-port/00-CONTRACT.md` §3).
//! All enums are `i32` with the Slint numbering verbatim
//! (`state.slint:4238,4249,4254`): view_mode 0 FOCUS / 1 SPLIT; mode 0 Album
//! Reactive, 1 Static, 2 Coverflow, 3 Spectrum, 4 Lyrics, 5 Queue, 6 WaveBed,
//! 7 Goniometer, 8 Oscilloscope, 9 Reactive Rings;
//! split_panel 0 Lyrics, 1 TrackInfo, 2 Suggestions, 3 Queue, 4 cinematic
//! Now Playing, 5 cinematic one-line Lyrics.
//!
//! The `open` property uses a CUSTOM WRITE (`set_open`) so EVERY write — the
//! five QML exit paths AND the Rust invokables — funnels through one Rust
//! setter (contract §3/D3: Qt's answer to Slint's 6 write sites + 150 ms
//! guard timer). The false→true edge runs §3.1 apply-default-view; both edges
//! drive the viz two-source enable (§3.3a/§4.2).
//!
//! The §3.4 search surface is wired to the immersive search controller in
//! `search_qt.rs` (B5): `search_live` mirrors the text AND runs the gate ->
//! 2-char gate -> 220 ms debounce -> version-guarded load pipeline;
//! `search_move_selection` clamps the keyboard selection; `search_row_activated`
//! runs the (kind x action) playback dispatch table; `dismiss_search` clears
//! open+selection. The controller publishes ONLY on this bridge's immSearch*
//! properties — never on QbzBridge.cortinilla* (contract §3.4).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

/// Full-shape default for the search document (contract §3.4 payload: a
/// `sections` array, NEVER "{}" — trap 15). B5 fills the section schema when
/// the controller lands.
const IMM_SEARCH_EMPTY: &str = r#"{"sections":[]}"#;

/// Full-shape default for the atmosphere-independent docs is not needed, but
/// the glow default IS a color Qt must parse: Qt colors are `#AARRGGBB`, so
/// the Slint default `#6464ff59` (RRGGBBAA, `state.slint:4231`) is stored
/// converted. `ambient_qt` publishes the same conversion — see its comment.
const GLOW_DEFAULT_QT: &str = "#596464ff";

#[cxx_qt::bridge]
pub mod qbz_immersive {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // Overlay open flag. CUSTOM WRITE (the `set_open` funnel below) —
        // QML writes `QbzImmersive.open = false` on every exit path and the
        // same edge logic runs (contract §3/D3).
        #[qproperty(bool, open, READ, WRITE = set_open, NOTIFY)]
        // 0 FOCUS / 1 SPLIT. Mutated ONLY by `setView` / the open-edge
        // restore — QML never writes these directly (§3.2).
        #[qproperty(i32, view_mode)]
        // FOCUS foreground: 0 AlbumReactive, 1 Static, 2 Coverflow, 3
        // Spectrum, 4 Lyrics, 5 Queue, 6 WaveBed, 7 Goniometer,
        // 8 Oscilloscope, 9 Reactive Rings.
        #[qproperty(i32, mode)]
        // SPLIT active panel: 0 Lyrics, 1 TrackInfo, 2 Suggestions, 3 Queue,
        // 4 cinematic Now Playing, 5 cinematic one-line Lyrics.
        #[qproperty(i32, split_panel)]
        // AlbumReactive glow, Qt `#AARRGGBB` (Slint `#6464ff59` RRGGBBAA
        // converted — see GLOW_DEFAULT_QT). Published by ambient_qt only on
        // success, never cleared (§4.3).
        #[qproperty(QString, glow_color)]
        // 128×128 atmosphere PNG as a file:// URL. Published ONLY when a
        // fresh asset is ready — NEVER cleared on track change (the flicker
        // fix, playback.rs:2344-2351 semantics; §4.3).
        #[qproperty(QString, atmosphere_url)]
        // --- Immersive search surface (§3.4; controller: search_qt.rs) -----
        #[qproperty(bool, imm_search_open)]
        // Sections Artists/Albums/Playlists IN THAT ORDER (+ a local-album
        // section, always fetched) — NO track rows, NO top result. Full-shape
        // default, never "{}".
        #[qproperty(QString, imm_search_json)]
        #[qproperty(bool, imm_search_loading)]
        // -1 = no selection (Down from -1 selects the first row; no wrap).
        #[qproperty(i32, imm_search_selected_index)]
        #[qproperty(f64, imm_search_scroll_y)]
        // Two-way with the search field.
        #[qproperty(QString, search_input_text)]
        type QbzImmersive = super::QbzImmersiveRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY
        /// domain singleton; only QbzSession.boot also fires crate::on_boot).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzImmersive>);

        /// Shift+I path (§3.3): flips `open`; does NOT force mode=0
        /// (1:1 keybindings.rs:538-545).
        #[qinvokable]
        fn toggle(self: Pin<&mut QbzImmersive>);

        /// NPB ViewModeMenu path (§3.3): FIRST setView(current view_mode, 0,
        /// current split_panel), THEN open=true.
        #[qinvokable]
        fn open_from_menu(self: Pin<&mut QbzImmersive>);

        /// The ONLY mode mutator (§3.2): sets the three properties and
        /// persists the triple ONLY when immersive_default_view == "remember".
        #[qinvokable]
        fn set_view(self: Pin<&mut QbzImmersive>, view_mode: i32, mode: i32, split_panel: i32);

        /// §3.4 search surface: mirrors the text into search_input_text AND
        /// runs the immersive controller (pref gate, >=2-char gate, 220 ms
        /// debounce, version-guarded load — search_qt::imm_live).
        #[qinvokable]
        fn search_live(self: Pin<&mut QbzImmersive>, text: QString);
        /// §3.4: arrows clamp both ends, Down from -1 -> first row, no wrap
        /// (search_qt::imm_move_selection + the 56/24/4 scroll arithmetic).
        #[qinvokable]
        fn search_move_selection(self: Pin<&mut QbzImmersive>, delta: i32);
        /// §3.4: the per-row-kind x action playback dispatch table — the user
        /// STAYS in immersive, the field clears (search_qt::imm_row_activated).
        #[qinvokable]
        fn search_row_activated(self: Pin<&mut QbzImmersive>, flat_index: i32);
        /// §3.4: clears imm_search_open + the selection.
        #[qinvokable]
        fn dismiss_search(self: Pin<&mut QbzImmersive>);
    }

    // The custom WRITE target of the `open` property. NOT a qinvokable — QML
    // reaches it by writing `QbzImmersive.open`, never by name. (cxx-qt
    // custom-setter pattern: the property's auto setter is replaced by this
    // method, which owns the store + notify + edge logic.)
    unsafe extern "RustQt" {
        fn set_open(self: Pin<&mut QbzImmersive>, value: bool);
    }

    impl cxx_qt::Threading for QbzImmersive {}
}

use qbz_immersive::QbzImmersive;

/// Rust side of the immersive bridge (plain storage, phase-1 pattern).
pub struct QbzImmersiveRust {
    open: bool,
    view_mode: i32,
    mode: i32,
    split_panel: i32,
    glow_color: QString,
    atmosphere_url: QString,
    imm_search_open: bool,
    imm_search_json: QString,
    imm_search_loading: bool,
    imm_search_selected_index: i32,
    imm_search_scroll_y: f64,
    search_input_text: QString,
}

impl Default for QbzImmersiveRust {
    fn default() -> Self {
        Self {
            open: false,
            // Pre-first-open construction default — the same Split > Lyrics
            // the remember-restore falls back to (see the DEFAULT_* consts).
            view_mode: DEFAULT_VIEW_MODE,
            mode: DEFAULT_MODE,
            split_panel: DEFAULT_SPLIT_PANEL,
            glow_color: QString::from(GLOW_DEFAULT_QT),
            atmosphere_url: QString::from(""),
            imm_search_open: false,
            imm_search_json: QString::from(IMM_SEARCH_EMPTY),
            imm_search_loading: false,
            imm_search_selected_index: -1,
            imm_search_scroll_y: 0.0,
            search_input_text: QString::from(""),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (viz_bridge.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzImmersive>> = OnceLock::new();

/// Queue an immersive-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzImmersive>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

/// Rust-side mirror of the `open` property, written inside `apply_open` — the
/// funnel EVERY write passes through (the custom-WRITE `set_open`, contract
/// §3/D3) — so it is exact. Read by the hotkeys pipeline: the (B)
/// dropdown-steal condition and the §1.2 Escape stack / the
/// `Context::Immersive` binding gate (2026-08-03 hotkeys-port contract),
/// which run on ANOTHER singleton and cannot reach this QObject's properties.
static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn is_open() -> bool {
    OPEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Cross-singleton `toggle()` — the hotkeys `ui.focusMode` dispatch
/// (keybindings.rs:538-545 semantics ride the QbzImmersive invokable itself).
pub(crate) fn toggle() {
    ui(|imm| imm.toggle());
}

/// Cross-singleton close — the hotkeys §1.2 Escape-stack arm 5. Rides the
/// SAME `set_open` funnel the QML exit paths use (the false edge parks the
/// viz tap, §3.3a/§4.2).
pub(crate) fn close() {
    ui(|imm| imm.set_open(false));
}

/// Owner ruling 2026-08-15: "cuando no hay una escena activa por default
/// debemos caer siempre en Split>Lyrics" — with NOTHING remembered, a fresh
/// immersive open lands on viewMode=1 (SPLIT), splitPanel=0 (Lyrics), never
/// on a FOCUS panel. These are the FALLBACKS of the remember-last restore
/// (and the bridge's construction default): a saved triple still wins, so
/// existing users keep their remembered view.
const DEFAULT_VIEW_MODE: i32 = 1; // SPLIT
const DEFAULT_MODE: i32 = 0; // (meaningless under SPLIT; FOCUS AlbumReactive)
const DEFAULT_SPLIT_PANEL: i32 = 0; // Lyrics

/// §3.1 pinned map: a pinned default always lands on `view_mode=0` + the
/// matching FOCUS `mode` (never WaveBed). `None` = "remember"/unknown — the
/// caller restores the persisted triple / does nothing respectively.
fn pinned_view(default_key: &str) -> Option<(i32, i32)> {
    match default_key {
        "reactive" => Some((0, 0)),
        "static" => Some((0, 1)),
        "coverflow" => Some((0, 2)),
        "spectrum" => Some((0, 3)),
        "lyrics" => Some((0, 4)),
        "queue" => Some((0, 5)),
        _ => None,
    }
}

/// The two stereo scopes are native scene-graph traces. On Qt's Software and
/// Null backends only their QML guide axes survive, so a remembered or stale
/// request must land on the compatible Spectrum panel instead. This is a live
/// resolution only: the remembered mode remains stored and returns on GPU.
fn focus_mode_for_tier(view_mode: i32, mode: i32, gpu_tier: bool) -> i32 {
    if view_mode == 0 && !gpu_tier && matches!(mode, 7 | 8) {
        3
    } else {
        mode
    }
}

/// The single funnel behind every `open` write (contract §3/D3). Holds the
/// store + notify, the §3.1 apply-default-view on the false→true edge, and
/// the §3.3a viz keep-alive on both edges.
fn apply_open(mut this: Pin<&mut QbzImmersive>, value: bool) {
    use cxx_qt::CxxQtType as _;
    if this.open == value {
        return;
    }
    this.as_mut().rust_mut().open = value;
    this.as_mut().open_changed();
    OPEN.store(value, std::sync::atomic::Ordering::SeqCst);
    if value {
        // §3.1 apply-default-view — runs on EVERY false→true open edge, from
        // both entries. Restore writes go DIRECTLY to the properties, NOT
        // through set_view (no re-persist on restore — §3.1). The Slint
        // shader-mode force-reset is a v1 no-op (§1.3).
        let key = crate::settings_qt::pref_str("immersive_default_view", "remember");
        if key == "remember" {
            // The remember-last triple. Stored as JSON NUMBERS: ui_prefs.json
            // is co-owned with the Slint app, whose typed ui_prefs struct
            // declares these keys i32 (crates/qbz/src/ui_prefs.rs:517-526) —
            // string writes would poison its whole-document load. (The
            // contract's "pref_str i32-parse" would read NEITHER app's
            // numbers; pref_i32 is the compatible reader — delivery notes.)
            let vm = crate::settings_qt::pref_i32("immersive_last_view_mode", DEFAULT_VIEW_MODE);
            let m = crate::settings_qt::pref_i32("immersive_last_mode", DEFAULT_MODE);
            let sp =
                crate::settings_qt::pref_i32("immersive_last_split_panel", DEFAULT_SPLIT_PANEL);
            this.as_mut().set_view_mode(vm);
            this.as_mut().set_mode(m);
            this.as_mut().set_split_panel(sp);
        } else if let Some((vm, m)) = pinned_view(&key) {
            // Pinned: view_mode 0 + the mapped mode; split_panel untouched.
            this.as_mut().set_view_mode(vm);
            this.as_mut().set_mode(m);
        }
        // Unknown key: Slint's `_ => {}` — leave the current view alone.
        let live_mode =
            focus_mode_for_tier(this.view_mode, this.mode, crate::renderer_qt::gpu_tier());
        if live_mode != this.mode {
            log::info!("[qbz-qt] immersive: native scope unavailable on renderer tier -> Spectrum");
            this.as_mut().set_mode(live_mode);
        }
        crate::viz_qt::immersive_opened();
        crate::viz_qt::set_immersive_view(this.view_mode, this.mode);
        // Shader scenes (block A1): the open edge decides the scene. Under
        // "remember" it restores the last one (2026-08-19 — the triple above
        // never covered the scenes, so Plasma / Ribbon / Line Bed / Tunnel
        // Flow were the views "remember last view" could not remember); under
        // a pinned default, off-tier, or with nothing shipped stored, it
        // force-resets to Off, which is the Slint behaviour
        // (main.rs:10300-10301) and stays the fallback.
        crate::shader_scene_bridge::apply_on_immersive_open();
        // §3.4 open-edge hygiene: a stale-true desktop cortinillaOpen is
        // possible in theory — close it deterministically (the Rust
        // equivalent of QbzBridge.cortinillaDismiss, bridge.rs:359-361; the
        // desktop Cortinilla must never float under the immersive overlay).
        crate::search_qt::dismiss();
    } else {
        crate::viz_qt::immersive_closed();
    }
}

impl qbz_immersive::QbzImmersive {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] immersive Qt thread already registered");
        }
    }

    /// The custom WRITE target of the `open` property — the funnel every
    /// QML `QbzImmersive.open = ...` write passes through (contract §3/D3).
    pub fn set_open(self: Pin<&mut Self>, value: bool) {
        apply_open(self, value);
    }

    /// Shift+I path (§3.3): flips `open`; mode NOT forced.
    pub fn toggle(mut self: Pin<&mut Self>) {
        let next = !self.open;
        apply_open(self.as_mut(), next);
    }

    /// NPB ViewModeMenu path (§3.3, round-2 corrected): writes mode=0
    /// DIRECTLY to the property (NO persist — Slint's `set_mode(0)` fires
    /// no view-sig because ImmersiveView is unmounted while closed), then
    /// open=true. The §3.1 restore on the open edge then decides: under
    /// "remember" the remembered triple wins verbatim (even mode 6 — that
    /// IS Slint: main.rs:11726 + 10296-10337); under a pinned pref §3.1
    /// forces the pin anyway. Routing this through setView would persist
    /// the construction defaults over the remembered triple on a fresh
    /// boot and destroy the feature.
    pub fn open_from_menu(mut self: Pin<&mut Self>) {
        self.as_mut().set_mode(0);
        apply_open(self.as_mut(), true);
    }

    /// The ONLY mode mutator (§3.2). Persists the triple via
    /// `settings_qt::save_pref` ONLY when immersive_default_view ==
    /// "remember"; a pinned default wins on the next open, so it must not be
    /// overwritten (main.rs:10339-10357 parity).
    pub fn set_view(mut self: Pin<&mut Self>, view_mode: i32, mode: i32, split_panel: i32) {
        let mode = focus_mode_for_tier(view_mode, mode, crate::renderer_qt::gpu_tier());
        self.as_mut().set_view_mode(view_mode);
        self.as_mut().set_mode(mode);
        self.as_mut().set_split_panel(split_panel);
        crate::viz_qt::set_immersive_view(view_mode, mode);
        if crate::settings_qt::pref_str("immersive_default_view", "remember") == "remember" {
            // JSON numbers — co-owned file, Slint declares the keys i32 (see
            // apply_open's comment).
            crate::settings_qt::save_pref(
                "immersive_last_view_mode",
                serde_json::Value::Number(view_mode.into()),
            );
            crate::settings_qt::save_pref(
                "immersive_last_mode",
                serde_json::Value::Number(mode.into()),
            );
            crate::settings_qt::save_pref(
                "immersive_last_split_panel",
                serde_json::Value::Number(split_panel.into()),
            );
        }
    }

    /// §3.4 live entry: mirror the text (two-way with the field), then hand
    /// the query to the immersive controller. The controller re-reads the
    /// `immersive_search_action` pref FRESH per keystroke and gates on it; the
    /// overlay-open gate reads THIS object's property (the qinvokable already
    /// runs on the Qt thread).
    pub fn search_live(mut self: Pin<&mut Self>, text: QString) {
        self.as_mut().set_search_input_text(text.clone());
        crate::search_qt::imm_live(self.open, text.to_string());
    }

    /// §3.4 arrow move — the controller's clamp (Down from -1 selects the
    /// first row, Up from the first row returns to -1, no wrap) + the 56/24/4
    /// scroll-y arithmetic (main.rs:10506-10548).
    pub fn search_move_selection(self: Pin<&mut Self>, delta: i32) {
        crate::search_qt::imm_move_selection(delta);
    }

    /// §3.4 row activation — the controller's (kind x action) playback
    /// dispatch table (main.rs:10579-10751).
    pub fn search_row_activated(self: Pin<&mut Self>, flat_index: i32) {
        crate::search_qt::imm_row_activated(flat_index);
    }

    /// §3.4 dismiss: clears the dropdown + the selection (main.rs:10552-10564).
    pub fn dismiss_search(self: Pin<&mut Self>) {
        crate::search_qt::imm_dismiss();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_map_matches_the_slint_open_table() {
        // main.rs:10311-10336: reactive/static/coverflow/spectrum/lyrics/queue
        // -> view_mode 0 + mode 0..5; a pinned view never lands on WaveBed.
        assert_eq!(pinned_view("reactive"), Some((0, 0)));
        assert_eq!(pinned_view("static"), Some((0, 1)));
        assert_eq!(pinned_view("coverflow"), Some((0, 2)));
        assert_eq!(pinned_view("spectrum"), Some((0, 3)));
        assert_eq!(pinned_view("lyrics"), Some((0, 4)));
        assert_eq!(pinned_view("queue"), Some((0, 5)));
    }

    #[test]
    fn fresh_open_defaults_to_split_lyrics() {
        // Owner ruling 2026-08-15: with no remembered triple, the
        // remember-restore fallbacks AND the bridge construction default land
        // on viewMode=1 (SPLIT), splitPanel=0 (Lyrics) — never a FOCUS panel.
        assert_eq!(
            (DEFAULT_VIEW_MODE, DEFAULT_MODE, DEFAULT_SPLIT_PANEL),
            (1, 0, 0)
        );
        assert_eq!(QbzImmersiveRust::default().view_mode, 1);
        assert_eq!(QbzImmersiveRust::default().split_panel, 0);
    }

    #[test]
    fn remember_and_unknown_keys_are_not_pinned() {
        assert_eq!(pinned_view("remember"), None);
        assert_eq!(pinned_view(""), None);
        assert_eq!(pinned_view("wavebed"), None); // pinned never lands on WaveBed
    }

    #[test]
    fn software_tier_falls_back_from_scopes_to_spectrum() {
        assert_eq!(focus_mode_for_tier(0, 7, false), 3);
        assert_eq!(focus_mode_for_tier(0, 8, false), 3);
        assert_eq!(focus_mode_for_tier(0, 7, true), 7);
        assert_eq!(focus_mode_for_tier(0, 8, true), 8);
        assert_eq!(focus_mode_for_tier(0, 6, false), 6);
        // FOCUS mode is irrelevant in Split; do not rewrite stored state.
        assert_eq!(focus_mode_for_tier(1, 8, false), 8);
    }

    #[test]
    fn search_doc_default_is_full_shape() {
        let doc: serde_json::Value = serde_json::from_str(IMM_SEARCH_EMPTY).unwrap();
        assert!(doc.get("sections").is_some_and(|s| s.is_array()));
    }

    #[test]
    fn glow_default_is_the_slint_default_in_qt_byte_order() {
        // Slint #6464ff59 (RRGGBBAA) -> Qt #AARRGGBB: alpha 0x59 first.
        assert_eq!(GLOW_DEFAULT_QT, "#596464ff");
    }
}
