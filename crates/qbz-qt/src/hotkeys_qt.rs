//! Keyboard / hotkeys layer — the PURE core (2026-08-03 hotkeys-port contract
//! `qbz-nix-docs/qt-frontend/2026-08-03-hotkeys-port/00-CONTRACT.md` v2 §3,
//! block B1). Port of the Slint `crates/qbz/src/keybindings.rs`
//! model: the DEFAULTS table (:88-117 — 23 actions at the time of the port;
//! NB its :3 "26 actions" comment is STALE, do not "fix" the count), the
//! shortcut-string grammar
//! (:153-249), the bindings store + conflict detection (:256-323), the groups
//! builder + three-column round-robin (:329-380), the capture semantics
//! (:434-467) and the §1.2 Escape priority stack (:585-615).
//!
//! Plain module — NO `#[cxx_qt::bridge]` here (the QbzHotkeys singleton lives
//! in hotkeys_bridge.rs; the core+bridge split is the suggestions_qt /
//! suggestions_bridge precedent). Do NOT list this file in build.rs
//! rust_files.
//!
//! TWO deliberate additions POST-port, both additive to the ported model:
//! eight Playback actions the Slint table never had (volume, mute, shuffle,
//! repeat, like, and a surface-independent seek — Spotify-inspired desktop
//! controls),
//! and the `Keymap` preset, which swaps the BASE shortcut table under the user
//! overrides instead of rewriting them. Everything below still resolves
//! through one action table and one canonical shortcut string.
//!
//! The one intentional DIFFERENCE from Slint is the token source (contract
//! §3.1 HYBRID rule, round-1 F1): Slint reads winit's LOGICAL key, which is
//! already the shifted glyph AND Ctrl-independent; a QKeyEvent's `text()` is
//! NOT (Ctrl+s yields U+0013, so a text-only rule breaks every Ctrl+printable
//! binding). Hence `token_from_qt_key` takes named keys AND letters/digits
//! from `event.key` (Qt key codes, letters uppercased under ShiftModifier —
//! reproducing winit's `Shift+s` -> "S", `Ctrl+s` -> "s") and consults
//! `event.text` ONLY for a single printable NON-control char (the
//! shifted-symbol case, `Shift+/` -> "?" — scoping trap 7). Bare modifiers,
//! multi-char and control texts yield None.
//!
//! Store discipline (contract §3.2): the SAME shared `ui_prefs.json` the
//! Slint app writes, ONE top-level `keybindings` key holding the whole
//! overrides map (action id -> shortcut). Reads go through the settings_qt
//! pref readers, writes ONLY through `settings_qt::save_pref("keybindings",
//! map)` — the additive-patch discipline of settings_qt.rs:135-158 holds
//! because the map is one key. Every pure function takes the overrides map as
//! an argument, so the §3.5 unit tests never touch the real file.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

// ============================================================================
// Qt key codes + modifier masks (Qt::Key / Qt::KeyboardModifier, qnamespace.h)
// ============================================================================
// Only the codes the grammar names. QML passes `event.key` / `event.modifiers`
// straight through, so these are the raw values Qt delivers.

pub(crate) const QT_KEY_ESCAPE: i32 = 0x0100_0000;
pub(crate) const QT_KEY_TAB: i32 = 0x0100_0001;
pub(crate) const QT_KEY_BACKSPACE: i32 = 0x0100_0003;
pub(crate) const QT_KEY_RETURN: i32 = 0x0100_0004;
pub(crate) const QT_KEY_ENTER: i32 = 0x0100_0005;
pub(crate) const QT_KEY_DELETE: i32 = 0x0100_0007;
pub(crate) const QT_KEY_LEFT: i32 = 0x0100_0012;
pub(crate) const QT_KEY_UP: i32 = 0x0100_0013;
pub(crate) const QT_KEY_RIGHT: i32 = 0x0100_0014;
pub(crate) const QT_KEY_DOWN: i32 = 0x0100_0015;
pub(crate) const QT_KEY_SPACE: i32 = 0x20;
pub(crate) const QT_KEY_0: i32 = 0x30;
pub(crate) const QT_KEY_9: i32 = 0x39;
pub(crate) const QT_KEY_A: i32 = 0x41;
pub(crate) const QT_KEY_Z: i32 = 0x5a;

pub(crate) const QT_SHIFT: i32 = 0x0200_0000;
pub(crate) const QT_CONTROL: i32 = 0x0400_0000;
pub(crate) const QT_ALT: i32 = 0x0800_0000;
pub(crate) const QT_META: i32 = 0x1000_0000;

/// `(ctrl, alt, shift)` from a QKeyEvent `modifiers` word — contract §1.1(0):
/// modifier state comes WITH each event, and Ctrl folds Meta (the TS
/// `ctrlKey || metaKey`, Slint `main.rs:1266-1274`'s
/// `control_key() || super_key()`).
pub fn mods_from_qt(modifiers: i32) -> (bool, bool, bool) {
    (
        modifiers & (QT_CONTROL | QT_META) != 0,
        modifiers & QT_ALT != 0,
        modifiers & QT_SHIFT != 0,
    )
}

// ============================================================================
// Action model (keybindings.rs:37-121 verbatim)
// ============================================================================

/// Display/grouping category. Order here is the on-screen order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Playback,
    Navigation,
    Ui,
    Immersive,
    Mini,
}

impl Category {
    const ORDER: [Category; 5] = [
        Category::Playback,
        Category::Navigation,
        Category::Ui,
        Category::Immersive,
        Category::Mini,
    ];

    /// English source string for the localized category header.
    fn label_en(self) -> &'static str {
        match self {
            Category::Playback => "Playback",
            Category::Navigation => "Navigation",
            Category::Ui => "Interface",
            Category::Immersive => "Immersive",
            Category::Mini => "Mini Player",
        }
    }
}

/// When an action only fires in a specific surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Context {
    None,
    /// Only while the immersive overlay is open (seek shortcuts).
    Immersive,
    /// Only while the miniplayer window is active (surfaces 0-4). Dispatched
    /// on the mini window's own hook — `QbzMini.keyPressed` through
    /// `action_for_mini_key` below — and NEVER on the main window, whose
    /// `action_for_key` still answers `Context::Mini => None` (there is a test
    /// on that arm). The two functions are siblings, not one widened gate.
    Mini,
}

/// Which preset supplies the base shortcuts. User overrides sit on top of
/// whichever base is active, so switching preset never discards them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Keymap {
    #[default]
    Default,
    /// Modifier-free home-row bindings. `j`/`k` are deliberately left UNBOUND:
    /// they belong to a per-list cursor, which QBZ has no notion of yet, and a
    /// global action bound there today would have to be taken back the day one
    /// lands.
    Vim,
}

impl Keymap {
    /// Persisted `ui_prefs.json` value. An unknown string reads as `Default` —
    /// a typo must not unbind the app.
    pub fn from_key(key: &str) -> Keymap {
        match key {
            "vim" => Keymap::Vim,
            _ => Keymap::Default,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Keymap::Default => "default",
            Keymap::Vim => "vim",
        }
    }

    /// QML passes the selector's index; same unknown-reads-as-Default rule.
    pub fn from_index(index: i32) -> Keymap {
        match index {
            1 => Keymap::Vim,
            _ => Keymap::Default,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Keymap::Default => 0,
            Keymap::Vim => 1,
        }
    }
}

pub struct ActionDef {
    pub id: &'static str,
    pub label_en: &'static str,
    pub category: Category,
    pub default: &'static str,
    /// Shortcut under `Keymap::Vim`; empty = the preset keeps `default`.
    pub vim: &'static str,
    pub context: Context,
}

impl ActionDef {
    /// This action's shortcut under `keymap`, BEFORE user overrides.
    pub fn base(&self, keymap: Keymap) -> &'static str {
        match keymap {
            Keymap::Vim if !self.vim.is_empty() => self.vim,
            _ => self.default,
        }
    }
}

/// The full action table — the ported Tauri `ACTIONS` array plus the additive
/// playback actions and optional Vim bases described above.
pub const ACTIONS: &[ActionDef] = &[
    // Playback
    ActionDef {
        id: "playback.toggle",
        label_en: "Play / Pause",
        category: Category::Playback,
        default: "Space",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "playback.next",
        label_en: "Next Track",
        category: Category::Playback,
        default: "Ctrl+ArrowRight",
        vim: "n",
        context: Context::None,
    },
    ActionDef {
        id: "playback.prev",
        label_en: "Previous Track",
        category: Category::Playback,
        default: "Ctrl+ArrowLeft",
        vim: "p",
        context: Context::None,
    },
    ActionDef {
        id: "playback.volumeUp",
        label_en: "Volume Up",
        category: Category::Playback,
        default: "Ctrl+ArrowUp",
        vim: "Shift+K",
        context: Context::None,
    },
    ActionDef {
        id: "playback.volumeDown",
        label_en: "Volume Down",
        category: Category::Playback,
        default: "Ctrl+ArrowDown",
        vim: "Shift+J",
        context: Context::None,
    },
    ActionDef {
        id: "playback.mute",
        label_en: "Mute / Unmute",
        category: Category::Playback,
        default: "m",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "playback.shuffle",
        label_en: "Toggle Shuffle",
        category: Category::Playback,
        default: "Ctrl+s",
        vim: "s",
        context: Context::None,
    },
    ActionDef {
        id: "playback.repeat",
        label_en: "Cycle Repeat",
        category: Category::Playback,
        default: "Ctrl+r",
        vim: "r",
        context: Context::None,
    },
    ActionDef {
        id: "playback.favorite",
        label_en: "Like Track",
        category: Category::Playback,
        default: "Alt+Shift+B",
        vim: "f",
        context: Context::None,
    },
    // Seek from ANY surface. The `focus.seek*` rows below stay bare arrows and
    // stay immersive-only: there the arrows are free, everywhere else they
    // belong to whatever the pointer is over.
    ActionDef {
        id: "playback.seekForward",
        label_en: "Seek Forward",
        category: Category::Playback,
        default: "Ctrl+Shift+ArrowRight",
        vim: "l",
        context: Context::None,
    },
    ActionDef {
        id: "playback.seekBack",
        label_en: "Seek Back",
        category: Category::Playback,
        default: "Ctrl+Shift+ArrowLeft",
        vim: "h",
        context: Context::None,
    },
    // Navigation
    ActionDef {
        id: "nav.back",
        label_en: "Go Back",
        category: Category::Navigation,
        default: "Alt+ArrowLeft",
        vim: "Ctrl+o",
        context: Context::None,
    },
    ActionDef {
        id: "nav.forward",
        label_en: "Go Forward",
        category: Category::Navigation,
        default: "Alt+ArrowRight",
        vim: "Ctrl+i",
        context: Context::None,
    },
    ActionDef {
        id: "nav.search",
        label_en: "Search",
        category: Category::Navigation,
        default: "Ctrl+f",
        vim: "/",
        context: Context::None,
    },
    ActionDef {
        id: "nav.settings",
        label_en: "Settings",
        category: Category::Navigation,
        default: "Ctrl+,",
        vim: "",
        context: Context::None,
    },
    // Interface
    ActionDef {
        id: "ui.sidebar",
        label_en: "Toggle Sidebar",
        category: Category::Ui,
        default: "Shift+S",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "ui.focusMode",
        label_en: "Immersive Mode",
        category: Category::Ui,
        default: "Shift+I",
        vim: "z",
        context: Context::None,
    },
    ActionDef {
        id: "ui.queue",
        label_en: "Queue",
        category: Category::Ui,
        default: "q",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "ui.escape",
        label_en: "Close / Dismiss",
        category: Category::Ui,
        default: "Escape",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "ui.showShortcuts",
        label_en: "Show Shortcuts",
        category: Category::Ui,
        default: "?",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "ui.openLink",
        label_en: "Open Music Link",
        category: Category::Ui,
        default: "Ctrl+l",
        vim: "",
        context: Context::None,
    },
    ActionDef {
        id: "ui.miniPlayer",
        label_en: "Toggle Mini Player",
        category: Category::Ui,
        default: "Shift+M",
        vim: "",
        context: Context::None,
    },
    // Immersive (contextual)
    ActionDef {
        id: "focus.seekForward",
        label_en: "Seek Forward (5s)",
        category: Category::Immersive,
        default: "ArrowRight",
        vim: "",
        context: Context::Immersive,
    },
    ActionDef {
        id: "focus.seekBack",
        label_en: "Seek Back (5s)",
        category: Category::Immersive,
        default: "ArrowLeft",
        vim: "",
        context: Context::Immersive,
    },
    ActionDef {
        id: "focus.seekForwardLong",
        label_en: "Seek Forward (10s)",
        category: Category::Immersive,
        default: "Shift+ArrowRight",
        vim: "",
        context: Context::Immersive,
    },
    ActionDef {
        id: "focus.seekBackLong",
        label_en: "Seek Back (10s)",
        category: Category::Immersive,
        default: "Shift+ArrowLeft",
        vim: "",
        context: Context::Immersive,
    },
    // Mini Player (contextual — dispatched on the mini window)
    ActionDef {
        id: "mini.micro",
        label_en: "Micro View",
        category: Category::Mini,
        default: "1",
        vim: "",
        context: Context::Mini,
    },
    ActionDef {
        id: "mini.compact",
        label_en: "Compact View",
        category: Category::Mini,
        default: "2",
        vim: "",
        context: Context::Mini,
    },
    ActionDef {
        id: "mini.artwork",
        label_en: "Artwork View",
        category: Category::Mini,
        default: "3",
        vim: "",
        context: Context::Mini,
    },
    ActionDef {
        id: "mini.queue",
        label_en: "Listen list",
        category: Category::Mini,
        default: "4",
        vim: "",
        context: Context::Mini,
    },
    ActionDef {
        id: "mini.lyrics",
        label_en: "Lyrics View",
        category: Category::Mini,
        default: "5",
        vim: "",
        context: Context::Mini,
    },
];

pub fn action(id: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.id == id)
}

// ============================================================================
// Shortcut-string grammar (port of eventToShortcut / formatShortcutDisplay,
// keybindings.rs:153-249, with the §3.1 HYBRID token source)
// ============================================================================

/// Normalize a Qt key event to a canonical key token (the part after the
/// modifiers). Returns `None` for bare modifier presses and unrepresentable
/// keys. HYBRID rule (contract §3.1):
/// - named keys from `event.key` (the Qt key code);
/// - letters/digits from `event.key`, the base char, uppercased when
///   ShiftModifier is held (reproduces winit's logical key: `Shift+s` -> "S",
///   `Ctrl+s` -> "s" — `event.text` would be U+0013 there);
/// - `event.text` ONLY when it is a single printable NON-control char (the
///   shifted-symbol case: `Shift+/` -> "?").
pub fn token_from_qt_key(key: i32, modifiers: i32, text: &str) -> Option<String> {
    match key {
        QT_KEY_SPACE => return Some("Space".into()),
        QT_KEY_LEFT => return Some("ArrowLeft".into()),
        QT_KEY_RIGHT => return Some("ArrowRight".into()),
        QT_KEY_UP => return Some("ArrowUp".into()),
        QT_KEY_DOWN => return Some("ArrowDown".into()),
        QT_KEY_ESCAPE => return Some("Escape".into()),
        QT_KEY_RETURN | QT_KEY_ENTER => return Some("Enter".into()),
        QT_KEY_TAB => return Some("Tab".into()),
        QT_KEY_BACKSPACE => return Some("Backspace".into()),
        QT_KEY_DELETE => return Some("Delete".into()),
        _ => {}
    }
    // Letters/digits from the key CODE — Ctrl-independent, unlike text().
    if (QT_KEY_A..=QT_KEY_Z).contains(&key) {
        let base = (b'a' + (key - QT_KEY_A) as u8) as char;
        return Some(
            if modifiers & QT_SHIFT != 0 {
                base.to_ascii_uppercase()
            } else {
                base
            }
            .to_string(),
        );
    }
    if (QT_KEY_0..=QT_KEY_9).contains(&key) {
        let base = (b'0' + (key - QT_KEY_0) as u8) as char;
        return Some(base.to_string());
    }
    // The text fallback: exactly one char, printable (non-control). This is
    // where the shifted symbols come from (`Shift+/` delivers "?").
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() || c.is_control() {
        return None;
    }
    Some(c.to_string())
}

/// Build the canonical shortcut string from modifiers + a key token
/// (keybindings.rs:177-198 verbatim): `[Ctrl+][Alt+][Shift+]Key`, with Shift
/// emitted ONLY for letters, digits and named (multi-char) keys — for a
/// symbol the Shift is already "consumed" by producing the glyph. Net stored
/// strings: `Shift+s` -> "Shift+S"; `Shift+/` -> "?"; `Ctrl+s` -> "Ctrl+s".
pub fn shortcut_from_parts(ctrl: bool, alt: bool, shift: bool, token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if ctrl {
        parts.push("Ctrl".into());
    }
    if alt {
        parts.push("Alt".into());
    }
    // Shift is emitted only for letters, digits, and named (multi-char) keys.
    let is_named = token.chars().count() > 1;
    let single = token.chars().next().unwrap();
    let is_letter = !is_named && single.is_ascii_alphabetic();
    let is_digit = !is_named && single.is_ascii_digit();
    if shift && (is_named || is_letter || is_digit) {
        parts.push("Shift".into());
    }
    parts.push(token.to_string());
    Some(parts.join("+"))
}

const KEY_DISPLAY: &[(&str, &str)] = &[
    // Solid triangles, not the thin Unicode arrows (←→↑↓) whose heads were
    // nearly invisible at keycap size — these render a clear, filled arrowhead.
    ("ArrowLeft", "◀"),
    ("ArrowRight", "▶"),
    ("ArrowUp", "▲"),
    ("ArrowDown", "▼"),
    ("Space", "Space"),
    ("Escape", "Esc"),
    ("Enter", "↵"),
    ("Backspace", "⌫"),
    ("Delete", "Del"),
    ("Tab", "Tab"),
];

/// Format a shortcut string for display (port of `formatShortcutDisplay`,
/// keybindings.rs:217-249 verbatim). macOS uses ⌘⌥⇧ glyphs joined by spaces;
/// elsewhere "Ctrl + …".
pub fn format_display(shortcut: &str) -> String {
    if shortcut.is_empty() {
        return String::new();
    }
    let (mut ctrl, mut alt, mut shift) = (false, false, false);
    let mut key = "";
    for part in shortcut.split('+') {
        match part {
            "Ctrl" => ctrl = true,
            "Alt" => alt = true,
            "Shift" => shift = true,
            other => key = other,
        }
    }
    let mac = cfg!(target_os = "macos");
    let mut out: Vec<String> = Vec::new();
    if ctrl {
        out.push(if mac { "⌘" } else { "Ctrl" }.into());
    }
    if alt {
        out.push(if mac { "⌥" } else { "Alt" }.into());
    }
    if shift {
        out.push(if mac { "⇧" } else { "Shift" }.into());
    }
    let disp = KEY_DISPLAY
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| (*v).to_string())
        .unwrap_or_else(|| key.to_uppercase());
    out.push(disp);
    out.join(if mac { " " } else { " + " })
}

// ============================================================================
// Bindings (defaults + user overrides) + conflict detection
// (keybindings.rs:256-323; storage contract §3.2)
// ============================================================================

/// The persisted user overrides (action id -> shortcut) from the shared
/// `ui_prefs.json` `keybindings` map — a NESTED JSON object, read via the
/// settings_qt `pref_json` reader (the map is one top-level key, so the
/// additive-patch discipline holds). A missing/unparsable key is an empty
/// map, NOT an error; non-string values are dropped defensively.
pub(crate) fn load_overrides() -> BTreeMap<String, String> {
    crate::settings_qt::pref_json("keybindings")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(id, sc)| sc.as_str().map(|s| (id, s.to_string())))
        .collect()
}

/// The active binding map: the `keymap` preset's bases overlaid with the
/// user's overrides (keybindings.rs:256-268). Overrides apply to KNOWN ids
/// only; an unknown id is silently ignored here but KEPT in the file (a stale
/// override rides along untouched — contract trap 3, RFB H5).
///
/// Overrides win over a base. A base shortcut some override has already taken
/// leaves ITS action UNBOUND ("") instead of double-binding the combo:
/// `action_for_shortcut` breaks a tie by BTreeMap id order, so a duplicate
/// would silently hand the key to whichever action sorts first. Only reachable
/// by switching preset with overrides in place — `apply_binding` refuses a
/// conflict outright — and the empty binding is visible as a blank keycap in
/// the editor rather than a wrong one.
pub fn active_bindings_with(
    keymap: Keymap,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut claimed: BTreeSet<&str> = overrides
        .iter()
        .filter(|(id, _)| action(id).is_some())
        .map(|(_, sc)| sc.as_str())
        .collect();
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for a in ACTIONS {
        if let Some(shortcut) = overrides.get(a.id) {
            map.insert(a.id.to_string(), shortcut.clone());
            continue;
        }
        let base = a.base(keymap);
        if claimed.contains(base) {
            map.insert(a.id.to_string(), String::new());
        } else {
            claimed.insert(base);
            map.insert(a.id.to_string(), base.to_string());
        }
    }
    map
}

/// The persisted preset (`ui_prefs.json` -> `keymap`), shared with the Slint
/// build the same way the `keybindings` map is.
pub fn active_keymap() -> Keymap {
    let key = crate::settings_qt::pref_json("keymap")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    Keymap::from_key(&key)
}

/// Persist the preset. User overrides are deliberately KEPT — they ride on top
/// of the new base (see `active_bindings_with`), so switching to Vim and back
/// is lossless.
pub fn set_keymap(keymap: Keymap) {
    crate::settings_qt::save_pref(
        "keymap",
        serde_json::Value::String(keymap.key().to_string()),
    );
}

fn action_for_shortcut<'a>(
    shortcut: &str,
    bindings: &'a BTreeMap<String, String>,
) -> Option<&'static ActionDef> {
    let id = bindings
        .iter()
        .find(|(_, v)| v.as_str() == shortcut)
        .map(|(k, _)| k.clone())?;
    action(&id)
}

/// The action (other than `exclude`) that already owns `shortcut`, if any —
/// conflict detection is EXACT canonical-string equality under a different id
/// (keybindings.rs:282-293, contract §3.3).
fn conflicting_action(
    shortcut: &str,
    exclude: &str,
    bindings: &BTreeMap<String, String>,
) -> Option<&'static ActionDef> {
    for (id, sc) in bindings {
        // "" is the unbound marker from `active_bindings_with`, never a combo.
        if !sc.is_empty() && sc == shortcut && id != exclude {
            return action(id);
        }
    }
    None
}

/// The pure half of `set_binding` (keybindings.rs:296-311): refuses (false,
/// map untouched) on a conflict; rebinding back to the default DROPS the
/// override (keeps the file minimal — back-to-default pruning, contract
/// trap 3). On true the caller persists the map.
pub fn apply_binding(
    keymap: Keymap,
    overrides: &mut BTreeMap<String, String>,
    action_id: &str,
    shortcut: &str,
) -> bool {
    let bindings = active_bindings_with(keymap, overrides);
    if conflicting_action(shortcut, action_id, &bindings).is_some() {
        return false;
    }
    // Back-to-BASE (not back-to-`default`) pruning: under Vim the preset's own
    // shortcut is what "unmodified" means, so rebinding to it drops the
    // override exactly as it does under Default.
    let base = action(action_id).map(|a| a.base(keymap));
    if Some(shortcut) == base {
        overrides.remove(action_id);
    } else {
        overrides.insert(action_id.to_string(), shortcut.to_string());
    }
    true
}

/// Persist the whole overrides map as the ONE `keybindings` top-level key
/// (the additive single-key patch — every other Slint key survives).
fn save_overrides(overrides: &BTreeMap<String, String>) {
    let map: serde_json::Map<String, serde_json::Value> = overrides
        .iter()
        .map(|(id, sc)| (id.clone(), serde_json::Value::String(sc.clone())))
        .collect();
    crate::settings_qt::save_pref("keybindings", serde_json::Value::Object(map));
}

/// Persist a new binding. Returns false (and writes nothing) on a conflict.
pub fn set_binding(action_id: &str, shortcut: &str) -> bool {
    let mut overrides = load_overrides();
    if !apply_binding(active_keymap(), &mut overrides, action_id, shortcut) {
        return false;
    }
    save_overrides(&overrides);
    true
}

pub fn reset_one(action_id: &str) {
    let mut overrides = load_overrides();
    overrides.remove(action_id);
    save_overrides(&overrides);
}

pub fn reset_all() {
    save_overrides(&BTreeMap::new());
}

// ============================================================================
// The groups model (cheatsheet + customize editor share it;
// keybindings.rs:329-380) — published to QML as ONE JSON document.
// ============================================================================

#[derive(Clone, Serialize)]
pub struct GroupRow {
    pub id: String,
    pub label: String,
    /// The FORMATTED display string (glyphs), as the Slint rows carry.
    pub shortcut: String,
    pub modified: bool,
    pub contextual: bool,
}

#[derive(Clone, Serialize)]
pub struct Group {
    pub label: String,
    pub rows: Vec<GroupRow>,
}

/// The 5 groups in `Category::ORDER`, labels localized via `qbz_i18n::t`
/// (fallback = the English msgid — the inherited gap, contract §5; the 20
/// missing msgids were added to the 7 non-en catalogs in this same block).
pub fn build_groups(keymap: Keymap, overrides: &BTreeMap<String, String>) -> Vec<Group> {
    let bindings = active_bindings_with(keymap, overrides);
    let mut groups: Vec<Group> = Vec::new();
    for cat in Category::ORDER {
        let mut rows: Vec<GroupRow> = Vec::new();
        for a in ACTIONS.iter().filter(|a| a.category == cat) {
            let shortcut = bindings.get(a.id).cloned().unwrap_or_default();
            // A preset collision can leave this base action visibly unbound
            // even though the user never edited it. Only a meaningful stored
            // override gets a reset affordance; otherwise resetOne would be a
            // button that cannot change the row.
            let modified = overrides
                .get(a.id)
                .is_some_and(|shortcut| shortcut != a.base(keymap));
            rows.push(GroupRow {
                id: a.id.to_string(),
                label: qbz_i18n::t(a.label_en),
                shortcut: format_display(&shortcut),
                modified,
                contextual: a.context != Context::None,
            });
        }
        groups.push(Group {
            label: qbz_i18n::t(cat.label_en()),
            rows,
        });
    }
    groups
}

pub fn modified_count_with(keymap: Keymap, overrides: &BTreeMap<String, String>) -> i32 {
    ACTIONS
        .iter()
        .filter(|a| {
            overrides
                .get(a.id)
                .is_some_and(|shortcut| shortcut != a.base(keymap))
        })
        .count() as i32
}

/// The QML-facing document (contract §3.4): the 5 groups split round-robin
/// into THREE columns (`cols[i % 3]`, keybindings.rs:368-374) so the
/// cheatsheet/editor render three Repeater columns from one doc.
/// `{"col1":[Group…],"col2":[…],"col3":[…]}` — full shape ALWAYS (trap 15:
/// never "{}").
pub fn groups_json(keymap: Keymap, overrides: &BTreeMap<String, String>) -> String {
    let groups = build_groups(keymap, overrides);
    let mut cols: [Vec<Group>; 3] = Default::default();
    for (i, g) in groups.into_iter().enumerate() {
        cols[i % 3].push(g);
    }
    let [c0, c1, c2] = cols;
    serde_json::json!({ "col1": c0, "col2": c1, "col3": c2 }).to_string()
}

// ============================================================================
// Capture (the customize editor's "press a key" widget — keybindings.rs:
// 434-467, contract §3.3). Pure: the bridge applies the outcome to its
// properties + the store.
// ============================================================================

/// What one captured keypress did. Slint's `handle_capture` ALWAYS consumes
/// the event; the enum says what else happened.
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Escape — cancel recording, bind NOTHING (Escape stays the ui.escape
    /// default).
    Cancelled,
    /// Bare modifier / unrepresentable — ignore, KEEP recording.
    Ignored,
    /// The combo already belongs to another action: live `display` + the
    /// conflicting action's localized `label`; recording STAYS so the user
    /// can pick a different combo.
    Conflict { display: String, label: String },
    /// Clean — the caller runs `set_binding` + refresh + clears the capture
    /// state.
    Bound { shortcut: String },
}

pub fn capture_step(
    keymap: Keymap,
    overrides: &BTreeMap<String, String>,
    action_id: &str,
    key: i32,
    modifiers: i32,
    text: &str,
) -> CaptureOutcome {
    if key == QT_KEY_ESCAPE {
        return CaptureOutcome::Cancelled;
    }
    let (ctrl, alt, shift) = mods_from_qt(modifiers);
    let Some(token) = token_from_qt_key(key, modifiers, text) else {
        return CaptureOutcome::Ignored;
    };
    let Some(shortcut) = shortcut_from_parts(ctrl, alt, shift, &token) else {
        return CaptureOutcome::Ignored;
    };
    let bindings = active_bindings_with(keymap, overrides);
    if let Some(conflict) = conflicting_action(&shortcut, action_id, &bindings) {
        return CaptureOutcome::Conflict {
            display: format_display(&shortcut),
            label: qbz_i18n::t(conflict.label_en),
        };
    }
    CaptureOutcome::Bound { shortcut }
}

// ============================================================================
// (D) Binding lookup — the pure half of `dispatch` (keybindings.rs:476-500).
// ============================================================================

/// Resolve a key event to an action over the active bindings. `None` = miss
/// (propagate) OR context-gated out: `Context::Immersive` requires the
/// immersive overlay open (the §1.3 gate); `Context::Mini` never fires on the
/// main window (Slint `dispatch` Propagates it — keybindings.rs:494-495; the
/// mini.* actions are kept in the table for binding-file compat + the
/// cheatsheet, contract §2/K3).
pub fn action_for_key(
    keymap: Keymap,
    overrides: &BTreeMap<String, String>,
    key: i32,
    modifiers: i32,
    text: &str,
    immersive_open: bool,
) -> Option<&'static ActionDef> {
    let (ctrl, alt, shift) = mods_from_qt(modifiers);
    let token = token_from_qt_key(key, modifiers, text)?;
    let shortcut = shortcut_from_parts(ctrl, alt, shift, &token)?;
    let bindings = active_bindings_with(keymap, overrides);
    let action = action_for_shortcut(&shortcut, &bindings)?;
    match action.context {
        Context::Immersive if !immersive_open => None,
        Context::Mini => None,
        _ => Some(action),
    }
}

/// Resolve a key event over the MINI window's context — a SIBLING of
/// `action_for_key`, never a widening of it (2026-08-03 miniplayer/tray
/// contract A-16, §4.9).
///
/// `Context::Mini` and `Context::None` fire here; `Context::Immersive` does
/// not, because the immersive overlay lives on the main window, which is hidden
/// while the mini is up. This mirrors the reference's split: `dispatch_mini`
/// (`crates/qbz/src/keybindings.rs:623-648`) resolves over the same ACTIVE
/// bindings — user overrides respected — and its table is the mini actions plus
/// every context-free playback row.
///
/// **`action_for_key`'s `Context::Mini => None` arm (`:572`) does NOT change.**
/// The main window must keep rejecting mini actions and the test at
/// `immersive_actions_are_gated_on_the_overlay_and_mini_never_fires` asserts it.
pub fn action_for_mini_key(
    keymap: Keymap,
    overrides: &BTreeMap<String, String>,
    key: i32,
    modifiers: i32,
    text: &str,
) -> Option<&'static ActionDef> {
    let (ctrl, alt, shift) = mods_from_qt(modifiers);
    let token = token_from_qt_key(key, modifiers, text)?;
    let shortcut = shortcut_from_parts(ctrl, alt, shift, &token)?;
    let bindings = active_bindings_with(keymap, overrides);
    let action = action_for_shortcut(&shortcut, &bindings)?;
    match action.context {
        Context::Immersive => None,
        _ => Some(action),
    }
}

/// The (C2) Ctrl+A predicate, EXACTLY the Slint one (contract §4.6 / trap 8):
/// `ctrl && token.eq_ignore_ascii_case("a")` — other modifiers are NOT
/// excluded, so Ctrl+Shift+A and Ctrl+Alt+A also trigger.
/// `main.rs:1381-1386`.
pub fn is_ctrl_a(key: i32, modifiers: i32, text: &str) -> bool {
    let (ctrl, _, _) = mods_from_qt(modifiers);
    if !ctrl {
        return false;
    }
    token_from_qt_key(key, modifiers, text).is_some_and(|tok| tok.eq_ignore_ascii_case("a"))
}

// ============================================================================
// The §1.2 Escape priority stack, as a PURE ordered check over a state
// struct (keybindings.rs:585-615, contract §1.2). The bridge fills the state
// from the live surfaces and executes the returned target.
// ============================================================================

#[derive(Clone, Copy, Default, Debug)]
pub struct EscapeState {
    /// App-wide Open Music Link modal; first in the Escape priority stack.
    pub link_resolver_open: bool,
    pub customize_open: bool,
    pub cheatsheet_open: bool,
    /// The global musician modal (z 3200). Sits ABOVE `immersive_open` in this
    /// order on purpose: it paints above the immersive overlay by design —
    /// consumer #6 opens it from the immersive Track-info panel and the
    /// overlay deliberately stays up underneath — so Escape must take the
    /// modal first, or the one keypress would close the thing behind it and
    /// leave the modal floating over a dismissed view.
    pub musician_modal_open: bool,
    pub cortinilla_open: bool,
    pub immersive_open: bool,
    /// §4.6 seam: no Qt multi-select exit invokable exists yet (B2 adds
    /// `QbzShell.exitMultiSelect`) — always false in B1.
    pub multi_select_active: bool,
    pub queue_open: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EscapeTarget {
    LinkResolver,
    Customize,
    Cheatsheet,
    MusicianModal,
    Cortinilla,
    Immersive,
    MultiSelect,
    Queue,
    /// Nothing dismissable is open. Slint still CONSUMES the matched
    /// ui.escape (dispatch PreventDefaults on any fired action,
    /// keybindings.rs:498-499) — the bridge mirrors that.
    None,
}

/// First match wins, in the contract §1.2 order.
pub fn escape_target(s: &EscapeState) -> EscapeTarget {
    if s.link_resolver_open {
        return EscapeTarget::LinkResolver;
    }
    if s.customize_open {
        return EscapeTarget::Customize;
    }
    if s.cheatsheet_open {
        return EscapeTarget::Cheatsheet;
    }
    if s.musician_modal_open {
        return EscapeTarget::MusicianModal;
    }
    if s.cortinilla_open {
        return EscapeTarget::Cortinilla;
    }
    if s.immersive_open {
        return EscapeTarget::Immersive;
    }
    if s.multi_select_active {
        return EscapeTarget::MultiSelect;
    }
    if s.queue_open {
        return EscapeTarget::Queue;
    }
    EscapeTarget::None
}

// ============================================================================
// Unit tests (contract §3.5 — mandatory). All pure: no prefs file, no Qt.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overrides() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    // --- Grammar round-trips (§3.5) ---------------------------------------

    #[test]
    fn named_keys_come_from_the_key_code() {
        assert_eq!(
            token_from_qt_key(QT_KEY_SPACE, 0, " "),
            Some("Space".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_LEFT, 0, ""),
            Some("ArrowLeft".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_RIGHT, 0, ""),
            Some("ArrowRight".into())
        );
        assert_eq!(token_from_qt_key(QT_KEY_UP, 0, ""), Some("ArrowUp".into()));
        assert_eq!(
            token_from_qt_key(QT_KEY_DOWN, 0, ""),
            Some("ArrowDown".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_ESCAPE, 0, ""),
            Some("Escape".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_RETURN, 0, "\r"),
            Some("Enter".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_ENTER, 0, "\r"),
            Some("Enter".into())
        );
        assert_eq!(token_from_qt_key(QT_KEY_TAB, 0, "\t"), Some("Tab".into()));
        assert_eq!(
            token_from_qt_key(QT_KEY_BACKSPACE, 0, "\u{8}"),
            Some("Backspace".into())
        );
        assert_eq!(
            token_from_qt_key(QT_KEY_DELETE, 0, "\u{7f}"),
            Some("Delete".into())
        );
    }

    #[test]
    fn shift_slash_yields_question_mark_not_shift_slash() {
        // scoping trap 7: the shifted glyph comes from event.text, and Shift
        // is folded into the glyph — the stored string is "?".
        let token = token_from_qt_key(0x2f, QT_SHIFT, "?").unwrap();
        assert_eq!(token, "?");
        assert_eq!(
            shortcut_from_parts(false, false, true, &token),
            Some("?".into())
        );
    }

    #[test]
    fn shift_s_yields_shift_upper_s() {
        let token = token_from_qt_key(QT_KEY_A + 18, QT_SHIFT, "S").unwrap(); // Key_S
        assert_eq!(token, "S");
        assert_eq!(
            shortcut_from_parts(false, false, true, &token),
            Some("Shift+S".into())
        );
    }

    #[test]
    fn ctrl_s_yields_ctrl_s_not_ctrl_control_char() {
        // The HYBRID rule's raison d'être (round-1 F1): event.text under Ctrl
        // is U+0013; the token MUST come from the key code.
        let token = token_from_qt_key(QT_KEY_A + 18, QT_CONTROL, "\u{13}").unwrap();
        assert_eq!(token, "s");
        assert_eq!(
            shortcut_from_parts(true, false, false, &token),
            Some("Ctrl+s".into())
        );
    }

    #[test]
    fn meta_folds_into_ctrl() {
        let (ctrl, alt, shift) = mods_from_qt(QT_META);
        assert!(ctrl && !alt && !shift);
        let (ctrl, _, _) = mods_from_qt(QT_META | QT_CONTROL);
        assert!(ctrl);
    }

    #[test]
    fn bare_modifier_and_unrepresentable_text_yield_none() {
        // Qt.Key_Shift — a bare modifier press.
        assert_eq!(token_from_qt_key(0x0100_0020, QT_SHIFT, ""), None);
        // Multi-char text (compose / IME preview).
        assert_eq!(token_from_qt_key(0x0100_0020, 0, "ab"), None);
        // A lone control char in text.
        assert_eq!(token_from_qt_key(0x0100_0020, 0, "\u{13}"), None);
        // Unknown key, empty text.
        assert_eq!(token_from_qt_key(0x0100_00ff, 0, ""), None);
    }

    #[test]
    fn letters_and_digits_come_from_the_key_code() {
        assert_eq!(token_from_qt_key(QT_KEY_A, 0, "a"), Some("a".into()));
        assert_eq!(token_from_qt_key(QT_KEY_Z, QT_SHIFT, "Z"), Some("Z".into()));
        assert_eq!(token_from_qt_key(QT_KEY_0 + 5, 0, "5"), Some("5".into()));
        // Ctrl+comma: comma has NO control-char mapping in xkb (unlike
        // letters), so event.text IS "," — the text arm delivers the token
        // and the nav.settings default ("Ctrl+,") binds.
        assert_eq!(token_from_qt_key(0x2c, QT_CONTROL, ","), Some(",".into()));
        assert_eq!(
            shortcut_from_parts(true, false, false, ","),
            Some("Ctrl+,".into())
        );
    }

    /// `format_display` is PLATFORM-DEPENDENT (`:285-303`): macOS renders the
    /// modifier glyphs `⌘ ⌥ ⇧` joined by a space, everywhere else the words
    /// joined by `" + "`. The assertions therefore have to branch too — a
    /// single hard-coded expectation is a test that can only pass on one
    /// platform, and this one failed the first time the suite was ever run on
    /// the Mac mini (2026-08-04), long after the code it covers was correct.
    ///
    /// The modifier-free cases are the same on both and stay unbranched: they
    /// are what actually pins the glyph TABLE, which is what the test is named
    /// for.
    #[test]
    fn format_display_uses_the_glyph_table() {
        if cfg!(target_os = "macos") {
            assert_eq!(format_display("Ctrl+ArrowRight"), "⌘ ▶");
            assert_eq!(format_display("Shift+S"), "⇧ S");
            assert_eq!(format_display("Alt+ArrowLeft"), "⌥ ◀");
        } else {
            assert_eq!(format_display("Ctrl+ArrowRight"), "Ctrl + ▶");
            assert_eq!(format_display("Shift+S"), "Shift + S");
            assert_eq!(format_display("Alt+ArrowLeft"), "Alt + ◀");
        }
        assert_eq!(format_display("?"), "?");
        assert_eq!(format_display("Space"), "Space");
        assert_eq!(format_display("Escape"), "Esc");
        assert_eq!(format_display(""), "");
    }

    // --- Conflict detection (exact canonical-string equality) --------------

    #[test]
    fn conflict_is_exact_string_under_a_different_id() {
        let bindings = active_bindings_with(Keymap::Default, &no_overrides());
        // "q" is ui.queue's default — recording ui.sidebar sees the conflict.
        let conflict = conflicting_action("q", "ui.sidebar", &bindings).unwrap();
        assert_eq!(conflict.id, "ui.queue");
        // The OWNER of the combo is excluded (re-recording its own binding).
        assert!(conflicting_action("q", "ui.queue", &bindings).is_none());
        // Case matters — exact string equality, no fuzzy match.
        assert!(conflicting_action("Q", "ui.queue", &bindings).is_none());
    }

    // --- active_bindings overlay + stale-id ignore -------------------------

    #[test]
    fn active_bindings_overlay_applies_known_ids_and_ignores_stale_ones() {
        let mut overrides = no_overrides();
        overrides.insert("ui.queue".into(), "w".into());
        overrides.insert("stale.removed.action".into(), "x".into()); // unknown id
        let bindings = active_bindings_with(Keymap::Default, &overrides);
        assert_eq!(bindings.get("ui.queue").unwrap(), "w");
        // The known count is exactly the action table; the stale id is absent.
        assert_eq!(bindings.len(), ACTIONS.len());
        assert!(!bindings.contains_key("stale.removed.action"));
        // …but the overrides map itself is untouched (kept in the file).
        assert_eq!(overrides.get("stale.removed.action").unwrap(), "x");
    }

    // --- apply_binding: conflict refusal + back-to-default pruning ---------

    #[test]
    fn apply_binding_refuses_conflicts_without_touching_the_map() {
        let mut overrides = no_overrides();
        assert!(!apply_binding(Keymap::Default, &mut overrides, "ui.sidebar", "q"));
        assert!(overrides.is_empty());
    }

    #[test]
    fn apply_binding_back_to_default_prunes_the_override() {
        let mut overrides = no_overrides();
        overrides.insert("ui.queue".into(), "w".into());
        assert!(apply_binding(Keymap::Default, &mut overrides, "ui.queue", "q")); // "q" = default
        assert!(!overrides.contains_key("ui.queue"));
    }

    #[test]
    fn apply_binding_writes_a_clean_rebind() {
        let mut overrides = no_overrides();
        assert!(apply_binding(Keymap::Default, &mut overrides, "ui.queue", "w"));
        assert_eq!(overrides.get("ui.queue").unwrap(), "w");
        assert_eq!(modified_count_with(Keymap::Default, &overrides), 1);
    }

    // --- Round-robin split {Playback,Immersive}/{Navigation,Mini}/{Interface}

    #[test]
    fn groups_split_round_robin_into_three_columns() {
        let doc: serde_json::Value = serde_json::from_str(&groups_json(Keymap::Default, &no_overrides())).unwrap();
        let labels = |col: &str| -> Vec<String> {
            doc[col]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["label"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(labels("col1"), vec!["Playback", "Immersive"]);
        assert_eq!(labels("col2"), vec!["Navigation", "Mini Player"]);
        assert_eq!(labels("col3"), vec!["Interface"]);
    }

    #[test]
    fn groups_rows_carry_id_label_shortcut_modified_contextual() {
        let doc: serde_json::Value = serde_json::from_str(&groups_json(Keymap::Default, &no_overrides())).unwrap();
        let row = &doc["col1"][0]["rows"][0];
        assert_eq!(row["id"], "playback.toggle");
        assert_eq!(row["label"], "Play / Pause"); // en fallback (msgid)
        assert_eq!(row["shortcut"], "Space");
        assert_eq!(row["modified"], false);
        assert_eq!(row["contextual"], false);
        // The immersive seek rows are contextual.
        let seek = &doc["col1"][1]["rows"][0];
        assert_eq!(seek["id"], "focus.seekForward");
        assert_eq!(seek["contextual"], true);
        // Full shape, never "{}" (trap 15).
        for col in ["col1", "col2", "col3"] {
            assert!(doc[col].is_array());
        }
    }

    // --- Escape stack order (§1.2) -----------------------------------------

    #[test]
    fn escape_stack_walks_the_eight_surfaces_in_order() {
        #[allow(clippy::too_many_arguments)]
        fn s(
            link: bool,
            cust: bool,
            cheat: bool,
            musician: bool,
            cort: bool,
            imm: bool,
            multi: bool,
            queue: bool,
        ) -> EscapeState {
            EscapeState {
                link_resolver_open: link,
                customize_open: cust,
                cheatsheet_open: cheat,
                musician_modal_open: musician,
                cortinilla_open: cort,
                immersive_open: imm,
                multi_select_active: multi,
                queue_open: queue,
            }
        }
        // Everything open → the link-resolver arm wins (order position 1).
        assert_eq!(
            escape_target(&s(true, true, true, true, true, true, true, true)),
            EscapeTarget::LinkResolver
        );
        // Then, in order, each surface beats every LATER one.
        assert_eq!(
            escape_target(&s(false, true, true, true, true, true, true, true)),
            EscapeTarget::Customize
        );
        assert_eq!(
            escape_target(&s(false, false, true, true, true, true, true, true)),
            EscapeTarget::Cheatsheet
        );
        assert_eq!(
            escape_target(&s(false, false, false, true, true, true, true, true)),
            EscapeTarget::MusicianModal
        );
        assert_eq!(
            escape_target(&s(false, false, false, false, true, true, true, true)),
            EscapeTarget::Cortinilla
        );
        assert_eq!(
            escape_target(&s(false, false, false, false, false, true, true, true)),
            EscapeTarget::Immersive
        );
        assert_eq!(
            escape_target(&s(false, false, false, false, false, false, true, true)),
            EscapeTarget::MultiSelect
        );
        assert_eq!(
            escape_target(&s(false, false, false, false, false, false, false, true)),
            EscapeTarget::Queue
        );
        assert_eq!(
            escape_target(&s(false, false, false, false, false, false, false, false)),
            EscapeTarget::None
        );
        // Multi-select beats the queue (order position 7 vs 8).
        assert_eq!(
            escape_target(&s(false, false, false, false, false, false, true, true)),
            EscapeTarget::MultiSelect
        );
    }

    /// The musician modal is opened FROM the immersive Track-info panel
    /// (consumer #6) and paints above it at z 3200, while the immersive
    /// overlay deliberately stays up underneath. So with both up, Escape must
    /// take the MODAL — dismissing immersive first would leave the modal
    /// floating over a view the user never asked to close.
    #[test]
    fn musician_modal_beats_immersive() {
        let state = EscapeState {
            musician_modal_open: true,
            immersive_open: true,
            ..EscapeState::default()
        };
        assert_eq!(escape_target(&state), EscapeTarget::MusicianModal);
    }

    // --- Capture: Escape-cancels vs conflict-keeps-recording ---------------

    #[test]
    fn capture_escape_cancels_and_bare_modifiers_are_ignored() {
        let overrides = no_overrides();
        assert_eq!(
            capture_step(Keymap::Default, &overrides, "ui.queue", QT_KEY_ESCAPE, 0, ""),
            CaptureOutcome::Cancelled
        );
        // Qt.Key_Shift alone → Ignored (keep recording).
        assert_eq!(
            capture_step(Keymap::Default, &overrides, "ui.queue", 0x0100_0020, QT_SHIFT, ""),
            CaptureOutcome::Ignored
        );
    }

    #[test]
    fn capture_conflict_keeps_recording_with_the_live_display() {
        let overrides = no_overrides();
        // Recording ui.sidebar, the user presses "q" (ui.queue's binding).
        let key_q = QT_KEY_A + 16;
        match capture_step(Keymap::Default, &overrides, "ui.sidebar", key_q, 0, "q") {
            CaptureOutcome::Conflict { display, label } => {
                assert_eq!(display, "Q");
                assert_eq!(label, "Queue"); // en fallback msgid
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        // A clean combo binds.
        let key_w = QT_KEY_A + 22;
        assert_eq!(
            capture_step(Keymap::Default, &overrides, "ui.queue", key_w, 0, "w"),
            CaptureOutcome::Bound {
                shortcut: "w".into()
            }
        );
    }

    // --- (D) lookup: context gates -----------------------------------------

    #[test]
    fn immersive_actions_are_gated_on_the_overlay_and_mini_never_fires() {
        let overrides = no_overrides();
        // ArrowRight with immersive CLOSED → miss.
        assert!(action_for_key(Keymap::Default, &overrides, QT_KEY_RIGHT, 0, "", false).is_none());
        // …open → focus.seekForward.
        let a = action_for_key(Keymap::Default, &overrides, QT_KEY_RIGHT, 0, "", true).unwrap();
        assert_eq!(a.id, "focus.seekForward");
        // "1" is mini.micro — never on the main window.
        assert!(action_for_key(Keymap::Default, &overrides, QT_KEY_0 + 1, 0, "1", false).is_none());
        // A global action resolves regardless.
        let a = action_for_key(Keymap::Default, &overrides, QT_KEY_SPACE, 0, " ", false).unwrap();
        assert_eq!(a.id, "playback.toggle");
    }

    /// A-16's sibling gate: the mini window is the mirror image of the main
    /// one. Written beside its twin above ON PURPOSE — the pair is what proves
    /// the widening was NOT applied to the shared arm.
    #[test]
    fn the_mini_gate_is_the_mirror_of_the_main_one() {
        let overrides = no_overrides();
        // "1" is mini.micro — it fires HERE and nowhere else.
        let a = action_for_mini_key(Keymap::Default, &overrides, QT_KEY_0 + 1, 0, "1").unwrap();
        assert_eq!(a.id, "mini.micro");
        // …and it is still refused on the main window (the arm was not widened).
        assert!(action_for_key(Keymap::Default, &overrides, QT_KEY_0 + 1, 0, "1", false).is_none());
        // A global action resolves on both.
        let a = action_for_mini_key(Keymap::Default, &overrides, QT_KEY_SPACE, 0, " ").unwrap();
        assert_eq!(a.id, "playback.toggle");
        // Newly added context-free playback actions resolve here too; the
        // mini bridge owns a matching dispatch arm for each of them.
        let volume =
            action_for_mini_key(Keymap::Default, &overrides, QT_KEY_UP, QT_CONTROL, "").unwrap();
        assert_eq!(volume.id, "playback.volumeUp");
        let seek = action_for_mini_key(
            Keymap::Default,
            &overrides,
            QT_KEY_RIGHT,
            QT_CONTROL | QT_SHIFT,
            "",
        )
        .unwrap();
        assert_eq!(seek.id, "playback.seekForward");
        // Immersive-context actions never fire on the mini: the overlay lives
        // on the main window, which is hidden while the mini is up.
        assert!(action_for_mini_key(Keymap::Default, &overrides, QT_KEY_RIGHT, 0, "").is_none());
    }

    #[test]
    fn ctrl_a_predicate_matches_the_slint_rule() {
        let key_a = QT_KEY_A;
        assert!(is_ctrl_a(key_a, QT_CONTROL, ""));
        // Other modifiers are NOT excluded (§4.6 / trap 8).
        assert!(is_ctrl_a(key_a, QT_CONTROL | QT_SHIFT, "A"));
        assert!(is_ctrl_a(key_a, QT_CONTROL | QT_ALT, ""));
        // Meta folds into Ctrl.
        assert!(is_ctrl_a(key_a, QT_META, ""));
        // Bare "a" and Ctrl+b are out.
        assert!(!is_ctrl_a(key_a, 0, "a"));
        assert!(!is_ctrl_a(QT_KEY_A + 1, QT_CONTROL, ""));
    }

    // --- Keymap presets ----------------------------------------------------

    /// Every preset must be internally conflict-free: two actions sharing a
    /// combo would be resolved by BTreeMap id order, i.e. arbitrarily. This is
    /// the gate on adding a row to ACTIONS with a colliding `vim` (or
    /// `default`) shortcut — it fails at `cargo test`, not in someone's hands.
    #[test]
    fn every_preset_binds_each_shortcut_at_most_once() {
        for keymap in [Keymap::Default, Keymap::Vim] {
            let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
            for a in ACTIONS {
                let base = a.base(keymap);
                assert!(!base.is_empty(), "{} has no {keymap:?} shortcut", a.id);
                if let Some(prev) = seen.insert(base, a.id) {
                    panic!("{keymap:?}: {base:?} bound to both {prev} and {}", a.id);
                }
            }
        }
    }

    /// `j`/`k` stay free for a future per-list cursor — the reason the volume
    /// rows are Shift+J / Shift+K rather than the bare keys.
    #[test]
    fn vim_leaves_j_and_k_unbound() {
        let bound: Vec<&str> = ACTIONS.iter().map(|a| a.base(Keymap::Vim)).collect();
        assert!(!bound.contains(&"j"));
        assert!(!bound.contains(&"k"));
    }

    #[test]
    fn vim_swaps_the_bases_it_defines_and_inherits_the_rest() {
        let vim = active_bindings_with(Keymap::Vim, &no_overrides());
        assert_eq!(vim["nav.search"], "/");
        assert_eq!(vim["playback.seekForward"], "l");
        assert_eq!(vim["playback.volumeUp"], "Shift+K");
        // No `vim` column -> the default rides through unchanged.
        assert_eq!(vim["ui.queue"], "q");
        assert_eq!(vim["nav.settings"], "Ctrl+,");
        // Nothing counts as modified just because the preset changed.
        assert_eq!(modified_count_with(Keymap::Vim, &no_overrides()), 0);
    }

    #[test]
    fn vim_dispatches_its_own_bases() {
        let ov = no_overrides();
        let key_slash = 0x2f;
        assert!(action_for_key(Keymap::Default, &ov, key_slash, 0, "/", false).is_none());
        let a = action_for_key(Keymap::Vim, &ov, key_slash, 0, "/", false).unwrap();
        assert_eq!(a.id, "nav.search");
    }

    /// An override wins over the preset base, and the action whose base it took
    /// goes UNBOUND rather than sharing the combo. Reachable only by switching
    /// preset with overrides already in place.
    #[test]
    fn an_override_beats_a_base_and_unbinds_the_action_it_displaced() {
        let mut ov = no_overrides();
        ov.insert("ui.queue".into(), "/".into());
        let vim = active_bindings_with(Keymap::Vim, &ov);
        assert_eq!(vim["ui.queue"], "/");
        assert_eq!(vim["nav.search"], "", "displaced base must not double-bind");
        // The unbound row resolves to nothing rather than to an arbitrary
        // action, and "" never matches a real keypress.
        let a = action_for_key(Keymap::Vim, &ov, 0x2f, 0, "/", false).unwrap();
        assert_eq!(a.id, "ui.queue");
        // Only the row the user actually changed is resettable/countable. The
        // displaced search row has no override for resetOne to remove.
        let groups = build_groups(Keymap::Vim, &ov);
        let row = |id: &str| {
            groups
                .iter()
                .flat_map(|group| group.rows.iter())
                .find(|row| row.id == id)
                .unwrap()
        };
        assert!(row("ui.queue").modified);
        assert!(!row("nav.search").modified);
        assert_eq!(modified_count_with(Keymap::Vim, &ov), 1);
    }

    /// Rebinding to the ACTIVE preset's own shortcut prunes the override, the
    /// same way it does under Default.
    #[test]
    fn back_to_the_vim_base_prunes_the_override() {
        let mut ov = no_overrides();
        assert!(apply_binding(Keymap::Vim, &mut ov, "nav.search", "w"));
        assert_eq!(ov.get("nav.search").map(String::as_str), Some("w"));
        assert!(apply_binding(Keymap::Vim, &mut ov, "nav.search", "/"));
        assert!(!ov.contains_key("nav.search"), "back-to-base must prune");
    }

    #[test]
    fn unknown_keymap_keys_and_indexes_read_as_default() {
        assert_eq!(Keymap::from_key("emacs"), Keymap::Default);
        assert_eq!(Keymap::from_key(""), Keymap::Default);
        assert_eq!(Keymap::from_key("vim"), Keymap::Vim);
        assert_eq!(Keymap::from_index(7), Keymap::Default);
        assert_eq!(Keymap::from_index(Keymap::Vim.index()), Keymap::Vim);
        assert_eq!(Keymap::from_key(Keymap::Vim.key()), Keymap::Vim);
    }

    // --- The new Spotify-inspired actions ----------------------------------

    #[test]
    fn the_new_playback_actions_dispatch_on_their_defaults() {
        let ov = no_overrides();
        for (key, modifiers, text, id) in [
            (QT_KEY_UP, QT_CONTROL, "", "playback.volumeUp"),
            (QT_KEY_DOWN, QT_CONTROL, "", "playback.volumeDown"),
            (QT_KEY_A + 12, 0, "m", "playback.mute"),
            (QT_KEY_A + 18, QT_CONTROL, "", "playback.shuffle"),
            (QT_KEY_A + 17, QT_CONTROL, "", "playback.repeat"),
            (QT_KEY_A + 1, QT_ALT | QT_SHIFT, "", "playback.favorite"),
            (QT_KEY_RIGHT, QT_CONTROL | QT_SHIFT, "", "playback.seekForward"),
            (QT_KEY_LEFT, QT_CONTROL | QT_SHIFT, "", "playback.seekBack"),
        ] {
            let a = action_for_key(Keymap::Default, &ov, key, modifiers, text, false)
                .unwrap_or_else(|| panic!("{id} did not resolve"));
            assert_eq!(a.id, id);
        }
    }

    /// The global seek rows fire anywhere; the immersive ones keep their bare
    /// arrows and stay gated on the overlay.
    #[test]
    fn global_seek_is_context_free_and_immersive_seek_still_is_not() {
        let ov = no_overrides();
        let global = action_for_key(
            Keymap::Default,
            &ov,
            QT_KEY_RIGHT,
            QT_CONTROL | QT_SHIFT,
            "",
            false,
        )
        .unwrap();
        assert_eq!(global.id, "playback.seekForward");
        assert_eq!(global.context, Context::None);
        assert!(action_for_key(Keymap::Default, &ov, QT_KEY_RIGHT, 0, "", false).is_none());
        let imm = action_for_key(Keymap::Default, &ov, QT_KEY_RIGHT, 0, "", true).unwrap();
        assert_eq!(imm.id, "focus.seekForward");
    }
}
