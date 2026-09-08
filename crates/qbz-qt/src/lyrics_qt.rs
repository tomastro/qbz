//! Lyrics data + sync layer — drives the `qbz-lyrics` crate (LyricsService:
//! Qobuz primary + LRCLIB/OVH fallback chain + per-user SQLite cache, all
//! self-contained) and exposes the S4 sync math + the S5 display prefs to
//! QML through the `QbzLyrics` singleton.
//!
//! This is the Qt port of THREE Slint modules, kept in one file because they
//! share one document and one prefs record:
//!   - `crates/qbz/src/lyrics.rs`       — fetch rider, stale guard, translation
//!   - `crates/qbz/src/lyrics_sync.rs`  — active line + karaoke fraction
//!   - `crates/qbz/src/lyrics_prefs.rs` — per-user display prefs
//!
//! # Where the sync engine lives (deviation vs Slint, deliberate)
//!
//! Slint runs a Rust-side `slint::Timer` at ~30 Hz that PUSHES `active-index`
//! / `line-progress` into a global. cxx-qt has no equivalent UI-thread timer
//! primitive, and pumping 30 cross-thread `queue()` hops a second would be
//! strictly worse than what QML already gives us: a QML `Timer` on the UI
//! thread that PULLS three scalar invokables. So the cadence lives in
//! `qml/shell/LyricsSyncEngine.qml` and the MATH lives here — the same
//! `qbz_lyrics::sync` pure functions Slint calls, over the same Rust-held
//! parsed document (native Qobuz word stamps included; the JSON model that
//! crosses into QML carries no word data).
//!
//! # Position source
//!
//! Locally, [`position_ms`] reads `SharedState::current_position_ms()` — the
//! read-only ms getter on `qbz-player` (pure derivation from the existing
//! playback anchors; the audio path is untouched). The 1 Hz `npElapsedSecs`
//! poll is NOT used for lyrics: it is second-granular and would resurrect the
//! 1 Hz highlight this port is replacing.
//!
//! While a peer Qobuz Connect renderer owns playback (Q7, contract §11.1 —
//! port of `lyrics_sync.rs`'s REMOTE_* store), the sync invokables read the
//! REMOTE anchor instead: the playback poll's peer branch publishes the raw
//! renderer triple (position_ms / updated_at_ms / playing — MS end-to-end,
//! the facade's `RemoteNowPlaying` units) via [`publish_remote_anchor`] on
//! every tick, and [`position_ms`] extrapolates by `now_ms - updated_at_ms`
//! while playing, exactly like the bar's peer position. The remote gate is
//! the now-playing model's `is_remote` flag (`now_playing::is_remote()`);
//! the anchor itself clears from `now_playing::set_remote(false)` — the
//! single choke point every remote-end path funnels through (the qconnect
//! sink's badge refresh and the facade's disconnect tail, which the cast
//! suspend also rides).
//!
//! # Host surfaces
//! The desktop panel, Immersive focus/split modes and miniplayer all consume
//! this same document and sync math. "Lyrics copied" / "translation
//! unavailable" remain panel-local inline [`notice`] messages rather than
//! app-wide toasts.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;
use qbz_app::shell::AppRuntime;
use qbz_app::user_data::UserDataPaths;
use qbz_core::LoggingAdapter;
use qbz_lyrics::service::{LyricsOutcome, LyricsRequest, LyricsResponse, LyricsService};
use qbz_lyrics::LyricsDoc;
use qbz_models::QueueTrack;
use serde::{Deserialize, Serialize};

type Runtime = Arc<AppRuntime<LoggingAdapter>>;

// ---------------------------------------------------------------------------
// The QML singleton
// ---------------------------------------------------------------------------

#[cxx_qt::bridge]
pub mod qbz_lyrics_bridge {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // --- The document (Slint LyricsState.status/lines/synced/...) -----
        // ONE JSON doc: {status, synced, provider, error, translationAvailable,
        // lines:[{timeMs,endMs,text,translation}]}. `timeMs`/`endMs` are null
        // for unstamped lines. Word stamps never cross — the karaoke fraction
        // is computed Rust-side from the held document.
        #[qproperty(QString, doc_json)]
        // Header meta ("{title} - {artist}"), version-enriched like the bar.
        #[qproperty(QString, track_title)]
        #[qproperty(QString, track_artist)]
        // Translation (Qobuz API v10). `translation_available` gates the
        // floating toggle; `show_translation` is the render intent, flipped
        // by Rust ONLY when a translation actually arrived.
        #[qproperty(bool, translation_available)]
        #[qproperty(bool, show_translation)]
        // A translation refetch is in flight (the toggle shows a busy tint).
        #[qproperty(bool, translation_busy)]
        // Panel-local notice line: translation failures and the copy
        // confirmation land here. "" = hidden.
        #[qproperty(QString, notice)]
        // --- Display prefs (Slint LyricsState S5 block) -------------------
        // Persisted per user in <data_dir>/qbz/users/<id>/lyrics_prefs.json —
        // the SAME file and field values the Slint build writes, so a user
        // switching frontends keeps their settings.
        #[qproperty(bool, auto_follow)]
        // 0 System · 1 LINE Seed JP · 2 Montserrat · 3 Noto Sans · 4 Source Sans 3
        #[qproperty(i32, font_index)]
        // 0 S · 1 M · 2 L · 3 XL
        #[qproperty(i32, size_index)]
        // 0 off · 1 soft · 2 strong
        #[qproperty(i32, dimming_mode)]
        #[qproperty(bool, uppercase)]
        // Lite fill (perf): light the active line whole, no karaoke sweep.
        #[qproperty(bool, lite_fill)]
        // 0 Auto (account language) · 1..9 = en es fr de it pt nl ja ru
        #[qproperty(i32, translation_language_index)]
        // Active (karaoke fill) color: false = follow the theme accent.
        #[qproperty(bool, use_custom_color)]
        #[qproperty(QString, custom_color)]
        type QbzLyrics = super::QbzLyricsRust;

        /// Registers this object's Qt-thread hop and seeds the persisted
        /// prefs. Fired from LyricsPanel.qml's `Component.onCompleted`
        /// (the panel is instantiated by AppShell for the whole session).
        #[qinvokable]
        fn boot(self: Pin<&mut QbzLyrics>);

        // --- Sync engine (pulled by LyricsSyncEngine.qml) -----------------
        /// Millisecond playback position. Local: read-only derivation from
        /// the player's existing anchors — never a poll of the audio path.
        /// While a peer QConnect renderer owns playback (§11.1): the remote
        /// anchor, extrapolated by `now - updated_at_ms` when playing.
        #[qinvokable]
        fn position_ms(self: &QbzLyrics) -> f64;
        /// Effective playing flag (the engine's tick gate). Remote while a
        /// peer owns playback, like [`position_ms`].
        #[qinvokable]
        fn playing(self: &QbzLyrics) -> bool;
        /// Active line index at `now_ms`, or -1 (binary search,
        /// `qbz_lyrics::sync::find_active_line_index`).
        #[qinvokable]
        fn active_index_at(self: &QbzLyrics, now_ms: f64) -> i32;
        /// Karaoke clip fraction 0..1 of the line's rendered width —
        /// WORD-ANCHORED when the document carries native Qobuz wsync words,
        /// line-proportional otherwise (`sync::line_fill_fraction`).
        #[qinvokable]
        fn fill_fraction_at(self: &QbzLyrics, now_ms: f64, index: i32) -> f32;

        // --- Flyout actions -----------------------------------------------
        /// Persist the pref properties after any flyout mutation (the Slint
        /// `LyricsState.prefs-changed()` contract).
        #[qinvokable]
        fn prefs_changed(self: Pin<&mut QbzLyrics>);
        /// Restore the Tauri/Slint defaults and persist them.
        #[qinvokable]
        fn reset_prefs(self: Pin<&mut QbzLyrics>);
        /// Current document joined with `\n` — the flyout's "Copy lyrics"
        /// (QML performs the clipboard write; this shell has no Rust
        /// clipboard seam).
        #[qinvokable]
        fn plain_text(self: &QbzLyrics) -> QString;

        // --- Translation ---------------------------------------------------
        /// Floating toggle: ON refetches the current track with the resolved
        /// target language and flips `show_translation` only when a
        /// translation actually arrived; OFF is pure UI.
        #[qinvokable]
        fn toggle_translation(self: Pin<&mut QbzLyrics>);
        /// Dismiss the inline notice.
        #[qinvokable]
        fn clear_notice(self: Pin<&mut QbzLyrics>);
    }

    impl cxx_qt::Threading for QbzLyrics {}
}

use qbz_lyrics_bridge::QbzLyrics;

pub struct QbzLyricsRust {
    doc_json: QString,
    track_title: QString,
    track_artist: QString,
    translation_available: bool,
    show_translation: bool,
    translation_busy: bool,
    notice: QString,
    auto_follow: bool,
    font_index: i32,
    size_index: i32,
    dimming_mode: i32,
    uppercase: bool,
    lite_fill: bool,
    translation_language_index: i32,
    use_custom_color: bool,
    custom_color: QString,
}

impl Default for QbzLyricsRust {
    fn default() -> Self {
        let prefs = LyricsPrefs::default();
        Self {
            doc_json: QString::from("{}"),
            track_title: QString::default(),
            track_artist: QString::default(),
            translation_available: false,
            show_translation: false,
            translation_busy: false,
            notice: QString::default(),
            auto_follow: prefs.auto_follow,
            font_index: prefs.font_index(),
            size_index: prefs.size_index(),
            dimming_mode: prefs.dimming_index(),
            uppercase: prefs.uppercase,
            lite_fill: prefs.lite_fill,
            translation_language_index: prefs.translation_language_index(),
            use_custom_color: !prefs.active_color.is_empty(),
            custom_color: QString::from(prefs.swatch()),
        }
    }
}

static QT_THREAD: OnceLock<CxxQtThread<QbzLyrics>> = OnceLock::new();

/// Queue a lyrics-bridge mutation onto the Qt event loop (no-op before
/// `boot` registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzLyrics>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

// ---------------------------------------------------------------------------
// QConnect remote anchor (contract §11.1 — port of lyrics_sync.rs's REMOTE_*)
// ---------------------------------------------------------------------------

/// The peer renderer's RAW playback anchor, published by the playback poll's
/// peer branch on every tick while a peer owns playback (Slint
/// playback.rs:5142-5149). MS end-to-end — the facade's `RemoteNowPlaying`
/// units — until `position_ms()` extrapolates. Plain atomics, like the
/// reference: both sides touch them at low rates and a torn read across
/// fields self-corrects on the next tick. There is deliberately NO
/// lyrics-side ACTIVE flag (the Slint's `REMOTE_ACTIVE`): the getters gate
/// on `now_playing::is_remote()` instead, so the anchor's owner and the
/// gate's owner are the same model flag.
static REMOTE_POSITION_MS: AtomicU64 = AtomicU64::new(0);
static REMOTE_UPDATED_AT_MS: AtomicU64 = AtomicU64::new(0);
static REMOTE_PLAYING: AtomicBool = AtomicBool::new(false);

/// Wall-clock now in ms — the anchor extrapolation clock (the Slint
/// `now_ms`).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Publish the RAW peer-renderer anchor (NOT the already-extrapolated
/// position — the getter extrapolates at read time, like the reference's
/// engine tick). Called from the playback poll's peer branch on every tick
/// while a peer owns playback (`lyrics_sync::publish_remote_anchor`).
pub fn publish_remote_anchor(position_ms: u64, updated_at_ms: u64, playing: bool) {
    REMOTE_POSITION_MS.store(position_ms, Ordering::Relaxed);
    REMOTE_UPDATED_AT_MS.store(updated_at_ms, Ordering::Relaxed);
    REMOTE_PLAYING.store(playing, Ordering::Relaxed);
}

/// Drop the stored anchor. Called from `now_playing::set_remote(false)` —
/// the single remote-mode-ENDED choke point (see the note there). The
/// getters gate on that same model flag, so this is hygiene against a stale
/// first read on remote re-entry, not the gate itself.
pub fn clear_remote_anchor() {
    REMOTE_POSITION_MS.store(0, Ordering::Relaxed);
    REMOTE_UPDATED_AT_MS.store(0, Ordering::Relaxed);
    REMOTE_PLAYING.store(false, Ordering::Relaxed);
}

impl qbz_lyrics_bridge::QbzLyrics {
    pub fn boot(mut self: Pin<&mut Self>) {
        let thread = self.qt_thread();
        if QT_THREAD.set(thread).is_err() {
            // Expected when Main.qml also boots this singleton: re-seeding
            // the prefs below is the useful half and stays idempotent.
            log::debug!("[qbz-qt] lyrics Qt thread already registered");
        }
        let prefs = LyricsPrefs::load();
        self.as_mut().apply_prefs(&prefs);
    }

    // ---- sync engine ---------------------------------------------------

    pub fn position_ms(&self) -> f64 {
        // QConnect controller mode (§11.1): lyrics follow the PEER.
        // Extrapolate between the poll's anchor publishes exactly like the
        // reference's resolver (lyrics_sync.rs:176-183 — no duration clamp
        // there either).
        if crate::now_playing::is_remote() {
            let position_ms = REMOTE_POSITION_MS.load(Ordering::Relaxed);
            let updated_at_ms = REMOTE_UPDATED_AT_MS.load(Ordering::Relaxed);
            if REMOTE_PLAYING.load(Ordering::Relaxed) && updated_at_ms > 0 {
                return position_ms.saturating_add(now_ms().saturating_sub(updated_at_ms)) as f64;
            }
            return position_ms as f64;
        }
        match runtime_handle() {
            Some(runtime) => runtime.core().player().state.current_position_ms() as f64,
            None => 0.0,
        }
    }

    pub fn playing(&self) -> bool {
        if crate::now_playing::is_remote() {
            return REMOTE_PLAYING.load(Ordering::Relaxed);
        }
        match runtime_handle() {
            Some(runtime) => runtime.core().player().state.is_playing(),
            None => false,
        }
    }

    pub fn active_index_at(&self, now_ms: f64) -> i32 {
        let now = now_ms.max(0.0) as i64;
        with_current_doc(|doc| match doc {
            Some(doc) if doc.synced && !doc.lines.is_empty() => {
                qbz_lyrics::sync::find_active_line_index(&doc.lines, now)
            }
            _ => -1,
        })
    }

    pub fn fill_fraction_at(&self, now_ms: f64, index: i32) -> f32 {
        if index < 0 {
            return 0.0;
        }
        let now = now_ms.max(0.0) as i64;
        with_current_doc(|doc| match doc {
            Some(doc) if doc.synced && !doc.lines.is_empty() => {
                qbz_lyrics::sync::line_fill_fraction(&doc.lines, index as usize, now)
            }
            _ => 0.0,
        })
    }

    // ---- flyout --------------------------------------------------------

    pub fn prefs_changed(mut self: Pin<&mut Self>) {
        let prefs = LyricsPrefs::from_ui(&self);
        prefs.save();
        // The translation-language row must not be a dead control while the
        // toggle is ON: changing the target re-requests the current track
        // with the new language (the Slint build reaches the same result via
        // the next track change; here it is immediate).
        let language_index = prefs.translation_language_index();
        if translation_enabled()
            && language_index != LAST_LANGUAGE.swap(language_index, Ordering::SeqCst)
        {
            self.as_mut().set_translation_busy(true);
            let pref = prefs.translation_language.clone();
            crate::spawn(async move {
                enable_translation(pref).await;
            });
        } else {
            LAST_LANGUAGE.store(language_index, Ordering::SeqCst);
        }
    }

    pub fn reset_prefs(mut self: Pin<&mut Self>) {
        let prefs = LyricsPrefs::default();
        prefs.save();
        self.as_mut().apply_prefs(&prefs);
    }

    pub fn plain_text(&self) -> QString {
        let text = with_current_doc(|doc| doc.map(|d| d.plain_text()).unwrap_or_default());
        QString::from(text.as_str())
    }

    // ---- translation ---------------------------------------------------

    pub fn toggle_translation(mut self: Pin<&mut Self>) {
        if *self.show_translation() {
            // OFF is pure UI — the committed lines keep their (now hidden)
            // translation texts; no network (spec §B.3).
            set_translation_session(false, None);
            self.as_mut().set_show_translation(false);
            return;
        }
        if *self.translation_busy() {
            return;
        }
        let target_pref = TRANSLATION_LANGS
            .get((*self.translation_language_index()).clamp(0, 9) as usize)
            .copied()
            .unwrap_or("auto")
            .to_string();
        self.as_mut().set_translation_busy(true);
        self.as_mut().set_notice(QString::default());
        crate::spawn(async move {
            enable_translation(target_pref).await;
        });
    }

    pub fn clear_notice(mut self: Pin<&mut Self>) {
        self.as_mut().set_notice(QString::default());
    }

    // ---- internal ------------------------------------------------------

    fn apply_prefs(mut self: Pin<&mut Self>, prefs: &LyricsPrefs) {
        self.as_mut().set_auto_follow(prefs.auto_follow);
        self.as_mut().set_font_index(prefs.font_index());
        self.as_mut().set_size_index(prefs.size_index());
        self.as_mut().set_dimming_mode(prefs.dimming_index());
        self.as_mut().set_uppercase(prefs.uppercase);
        self.as_mut().set_lite_fill(prefs.lite_fill);
        self.as_mut()
            .set_translation_language_index(prefs.translation_language_index());
        self.as_mut()
            .set_use_custom_color(!prefs.active_color.is_empty());
        self.as_mut()
            .set_custom_color(QString::from(prefs.swatch()));
        LAST_LANGUAGE.store(prefs.translation_language_index(), Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Persisted display prefs (port of crates/qbz/src/lyrics_prefs.rs)
// ---------------------------------------------------------------------------

const FONTS: [&str; 5] = [
    "system",
    "line-seed-jp",
    "montserrat",
    "noto-sans",
    "source-sans-3",
];
const SIZES: [&str; 4] = ["small", "medium", "large", "xl"];
const DIMMINGS: [&str; 3] = ["off", "soft", "strong"];
/// Owner-approved translation targets: "auto" + the 9 ISO 639-1 codes; the
/// order matches the flyout's option list.
const TRANSLATION_LANGS: [&str; 10] =
    ["auto", "en", "es", "fr", "de", "it", "pt", "nl", "ja", "ru"];
/// Default swatch when the user has never picked one (the flyout's first).
const DEFAULT_SWATCH: &str = "#8b5cf6";

/// Same JSON shape and value strings as the Slint build's `lyrics_prefs.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LyricsPrefs {
    #[serde(default = "d_true")]
    auto_follow: bool,
    #[serde(default = "d_font")]
    font: String,
    #[serde(default = "d_size")]
    font_size: String,
    #[serde(default = "d_dimming")]
    dimming: String,
    #[serde(default)]
    active_color: String,
    #[serde(default)]
    uppercase: bool,
    #[serde(default)]
    lite_fill: bool,
    #[serde(default = "d_lang")]
    translation_language: String,
}

fn d_true() -> bool {
    true
}
fn d_font() -> String {
    "system".to_string()
}
fn d_size() -> String {
    "medium".to_string()
}
fn d_dimming() -> String {
    "strong".to_string()
}
fn d_lang() -> String {
    "auto".to_string()
}

impl Default for LyricsPrefs {
    fn default() -> Self {
        Self {
            auto_follow: true,
            font: d_font(),
            font_size: d_size(),
            dimming: d_dimming(),
            active_color: String::new(),
            uppercase: false,
            lite_fill: false,
            translation_language: d_lang(),
        }
    }
}

fn index_of(table: &[&str], value: &str, fallback: i32) -> i32 {
    table
        .iter()
        .position(|candidate| *candidate == value)
        .map(|i| i as i32)
        .unwrap_or(fallback)
}

impl LyricsPrefs {
    fn font_index(&self) -> i32 {
        index_of(&FONTS, &self.font, 0)
    }
    fn size_index(&self) -> i32 {
        index_of(&SIZES, &self.font_size, 1)
    }
    fn dimming_index(&self) -> i32 {
        index_of(&DIMMINGS, &self.dimming, 2)
    }
    fn translation_language_index(&self) -> i32 {
        index_of(&TRANSLATION_LANGS, &self.translation_language, 0)
    }
    /// The swatch the color row should preselect — the stored custom color,
    /// or the palette default while the theme accent is in use.
    fn swatch(&self) -> &str {
        if self.active_color.is_empty() {
            DEFAULT_SWATCH
        } else {
            &self.active_color
        }
    }

    fn path() -> Option<std::path::PathBuf> {
        let uid = UserDataPaths::load_last_user_id()?;
        UserDataPaths::data_dir_for(uid)
            .ok()
            .map(|dir| dir.join("lyrics_prefs.json"))
    }

    /// Load + sanitize (unknown values fall back to their default).
    fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        let mut prefs: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if !FONTS.contains(&prefs.font.as_str()) {
            prefs.font = d_font();
        }
        if !SIZES.contains(&prefs.font_size.as_str()) {
            prefs.font_size = d_size();
        }
        if !DIMMINGS.contains(&prefs.dimming.as_str()) {
            prefs.dimming = d_dimming();
        }
        if !TRANSLATION_LANGS.contains(&prefs.translation_language.as_str()) {
            prefs.translation_language = d_lang();
        }
        if !valid_color(&prefs.active_color) {
            prefs.active_color = String::new();
        }
        prefs
    }

    fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, text);
        }
    }

    /// Read the pref properties back off the singleton (the flyout writes
    /// them directly, then fires `prefsChanged()`).
    fn from_ui(bridge: &QbzLyrics) -> Self {
        let idx = |table: &[&str], i: i32, fallback: &str| -> String {
            table
                .get(i.max(0) as usize)
                .copied()
                .unwrap_or(fallback)
                .to_string()
        };
        let color = bridge.custom_color().to_string();
        Self {
            auto_follow: *bridge.auto_follow(),
            font: idx(&FONTS, *bridge.font_index(), "system"),
            font_size: idx(&SIZES, *bridge.size_index(), "medium"),
            dimming: idx(&DIMMINGS, *bridge.dimming_mode(), "strong"),
            active_color: if *bridge.use_custom_color() && valid_color(&color) {
                color
            } else {
                String::new()
            },
            uppercase: *bridge.uppercase(),
            lite_fill: *bridge.lite_fill(),
            translation_language: idx(
                &TRANSLATION_LANGS,
                *bridge.translation_language_index(),
                "auto",
            ),
        }
    }
}

fn valid_color(value: &str) -> bool {
    value.is_empty()
        || (value.len() == 7
            && value.starts_with('#')
            && value[1..].chars().all(|c| c.is_ascii_hexdigit()))
}

// ---------------------------------------------------------------------------
// Document + service state
// ---------------------------------------------------------------------------

/// Serialized line — `timeMs`/`endMs` are null on unstamped (plain) lines.
#[derive(Clone, Serialize)]
pub struct LyricsLine {
    #[serde(rename = "timeMs")]
    pub time_ms: Option<i64>,
    #[serde(rename = "endMs")]
    pub end_ms: Option<i64>,
    pub text: String,
    /// The v10 translation of this line, 1:1 with `text`; "" when the doc
    /// carries none (or it was not requested).
    pub translation: String,
}

#[derive(Default, Serialize)]
pub struct LyricsDocJson {
    /// 0 idle / 1 loading / 2 ready / 3 not-found / 4 error / 5 offline-miss.
    pub status: i32,
    pub lines: Vec<LyricsLine>,
    pub synced: bool,
    pub provider: String,
    pub error: String,
}

static SERVICE: OnceLock<Arc<LyricsService>> = OnceLock::new();
/// The user id the cache is currently bound for ("" = unbound).
static CACHE_USER: Mutex<String> = Mutex::new(String::new());
/// A runtime handle captured on the first fetch, so the sync invokables can
/// read the player position without going through `crate::app()` (which
/// panics before the composition root is installed).
static RUNTIME: OnceLock<Runtime> = OnceLock::new();
/// The parsed document for the current track, held Rust-side so the native
/// Qobuz word stamps survive for the sync engine (the QML model carries text
/// + line bounds + the translation only).
static CURRENT_DOC: Mutex<Option<LyricsDoc>> = Mutex::new(None);
/// Track id the last request was issued for — the stale-response guard (F2).
static CURRENT_TRACK: AtomicI64 = AtomicI64::new(-1);
/// Active translation session: (enabled, resolved language).
static TRANSLATION: Mutex<(bool, Option<String>)> = Mutex::new((false, None));
/// Last persisted translation-language index, so `prefs_changed` can tell a
/// language switch apart from any other pref mutation.
static LAST_LANGUAGE: AtomicI32 = AtomicI32::new(0);

fn runtime_handle() -> Option<Runtime> {
    RUNTIME.get().cloned()
}

/// Read access to the current parsed document (the closure runs under the
/// lock — keep it short; it executes on the UI thread at the tick rate).
fn with_current_doc<R>(f: impl FnOnce(Option<&LyricsDoc>) -> R) -> R {
    match CURRENT_DOC.lock() {
        Ok(guard) => f(guard.as_ref()),
        Err(_) => f(None),
    }
}

fn set_translation_session(enabled: bool, language: Option<String>) {
    if let Ok(mut t) = TRANSLATION.lock() {
        *t = (enabled, language);
    }
}

fn translation_enabled() -> bool {
    TRANSLATION.lock().map(|t| t.0).unwrap_or(false)
}

/// Build (once) the service from the core's client clone, and bind the
/// per-user cache on first use / user change.
async fn service(runtime: &Runtime) -> Option<Arc<LyricsService>> {
    let _ = RUNTIME.set(Arc::clone(runtime));
    let service = match SERVICE.get() {
        Some(s) => Arc::clone(s),
        None => {
            let client_lock = runtime.core().client();
            let client = {
                let guard = client_lock.read().await;
                guard.as_ref().cloned()
            }?;
            let service = Arc::new(LyricsService::with_qobuz_client(Arc::new(client)));
            let _ = SERVICE.set(Arc::clone(&service));
            service
        }
    };
    // Bind the per-user cache (idempotent per user). The std MutexGuard must
    // NOT be held across the await (it would make the future !Send).
    let needs_init = UserDataPaths::load_last_user_id()
        .map(|uid| {
            let bound = CACHE_USER.lock().unwrap();
            *bound != uid.to_string()
        })
        .unwrap_or(false);
    if needs_init {
        if let Some(uid) = UserDataPaths::load_last_user_id() {
            if let Some(dir) =
                dirs::cache_dir().map(|d| d.join("qbz").join("users").join(uid.to_string()))
            {
                match service.init_at(&dir).await {
                    Ok(()) => {
                        log::info!("[qbz-qt] lyrics cache bound for user {uid}");
                        *CACHE_USER.lock().unwrap() = uid.to_string();
                        // A user switch also re-seeds the display prefs.
                        let prefs = LyricsPrefs::load();
                        ui(move |mut b| {
                            b.as_mut().apply_prefs(&prefs);
                        });
                    }
                    Err(e) => log::error!("[qbz-qt] lyrics cache init failed: {e}"),
                }
            }
        }
    }
    Some(service)
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

fn publish(doc: LyricsDocJson, translation_available: bool, show_translation: bool) {
    let json = serde_json::to_string(&doc).unwrap_or_else(|_| "{}".into());
    // The legacy QbzShell.lyricsJson mirror stays fed (the property is owned
    // by the shell bridge; other surfaces may still bind it).
    let shell_json = json.clone();
    crate::shell_bridge::ui(move |mut b| {
        b.as_mut()
            .set_lyrics_json(QString::from(shell_json.as_str()));
    });
    ui(move |mut b| {
        b.as_mut().set_doc_json(QString::from(json.as_str()));
        b.as_mut().set_translation_available(translation_available);
        b.as_mut().set_show_translation(show_translation);
        b.as_mut().set_translation_busy(false);
    });
}

/// Publish the loading state (status 1) — called on every track change.
pub fn publish_loading() {
    if let Ok(mut doc) = CURRENT_DOC.lock() {
        *doc = None;
    }
    publish(
        LyricsDocJson {
            status: 1,
            ..Default::default()
        },
        false,
        false,
    );
}

/// Publish the idle/empty state (track cleared).
pub fn publish_idle() {
    if let Ok(mut doc) = CURRENT_DOC.lock() {
        *doc = None;
    }
    CURRENT_TRACK.store(-1, Ordering::SeqCst);
    set_translation_session(false, None);
    publish(LyricsDocJson::default(), false, false);
    ui(|mut b| {
        b.as_mut().set_track_title(QString::default());
        b.as_mut().set_track_artist(QString::default());
        b.as_mut().set_notice(QString::default());
    });
}

fn publish_notice(text: String) {
    ui(move |mut b| {
        b.as_mut().set_notice(QString::from(text.as_str()));
        b.as_mut().set_translation_busy(false);
    });
}

/// Map a resolved response into the QML document. `show_translation` is the
/// intent AFTER this commit (only true when a translation actually landed).
fn commit(result: Result<LyricsResponse, String>, show_translation: bool) {
    match result {
        Ok(response) => match response.outcome {
            LyricsOutcome::Found(found) => {
                let doc = found.doc;
                let translation = doc.translation.as_deref();
                let lines: Vec<LyricsLine> = doc
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(i, line)| LyricsLine {
                        time_ms: line.time_ms,
                        end_ms: line.end_ms,
                        text: line.text.clone(),
                        // 1:1 with the rendered lines; a missing entry
                        // degrades to "" (fail soft).
                        translation: translation
                            .and_then(|t| t.lines.get(i))
                            .map(|l| l.text.clone())
                            .unwrap_or_default(),
                    })
                    .collect();
                let n = lines.len();
                let synced = doc.synced;
                let provider = doc.provider.as_str().to_string();
                let translation_available = !doc.translation_langs.is_empty();
                let has_words = doc.lines.iter().any(|l| l.words.is_some());
                log::info!(
                    "[qbz-qt] lyrics: {n} lines ({provider}, synced={synced}, wsync={has_words})"
                );
                if let Ok(mut held) = CURRENT_DOC.lock() {
                    *held = Some(doc);
                }
                publish(
                    LyricsDocJson {
                        status: if lines.is_empty() { 3 } else { 2 },
                        synced,
                        provider,
                        lines,
                        ..Default::default()
                    },
                    translation_available,
                    show_translation,
                );
            }
            LyricsOutcome::NotFound => {
                if let Ok(mut held) = CURRENT_DOC.lock() {
                    *held = None;
                }
                publish(
                    LyricsDocJson {
                        status: 3,
                        ..Default::default()
                    },
                    false,
                    false,
                );
            }
            LyricsOutcome::NotAvailableOffline => {
                if let Ok(mut held) = CURRENT_DOC.lock() {
                    *held = None;
                }
                publish(
                    LyricsDocJson {
                        status: 5,
                        ..Default::default()
                    },
                    false,
                    false,
                );
            }
        },
        Err(e) => {
            if let Ok(mut held) = CURRENT_DOC.lock() {
                *held = None;
            }
            publish(
                LyricsDocJson {
                    status: 4,
                    error: e,
                    ..Default::default()
                },
                false,
                false,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

/// Qobuz vs non-Qobuz from the queue track's source tag (`lyrics.rs`
/// `source_kind`): `qobuz_download` rows carry the REAL catalog id, so the
/// Qobuz primary applies to them too; local / ephemeral / Plex rows have
/// synthetic ids and go straight to the metadata-keyed fallback chain.
fn source_kind(track: &QueueTrack) -> qbz_lyrics::service::LyricsSourceKind {
    use qbz_lyrics::service::LyricsSourceKind;
    match track.source.as_deref() {
        Some("local") | Some("ephemeral") | Some("plex") => LyricsSourceKind::NonQobuz,
        _ => LyricsSourceKind::Qobuz,
    }
}

fn make_request(track: &QueueTrack, language: Option<String>) -> LyricsRequest {
    let source = source_kind(track);
    LyricsRequest {
        track_id: (source == qbz_lyrics::service::LyricsSourceKind::Qobuz).then_some(track.id),
        source,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: (!track.album.is_empty()).then(|| track.album.clone()),
        duration_secs: (track.duration_secs > 0).then_some(track.duration_secs),
        offline: crate::offline_fwd::engine().is_offline(),
        language,
    }
}

/// Version-enriched display title, like the player bar (F4: the RAW title
/// still goes to the engine — the fallback providers match on it).
fn display_title(track: &QueueTrack) -> String {
    match track.version.as_deref().filter(|v| !v.is_empty()) {
        Some(version) => format!("{} ({version})", track.title),
        None => track.title.clone(),
    }
}

/// Fetch lyrics for the new current track (called from `refresh_now_playing`
/// on every track change; the panel re-requests on open via the same path — a
/// cached doc returns immediately).
pub async fn load_for_track(runtime: &Runtime, track: &QueueTrack) {
    let track_id = track.id as i64;
    CURRENT_TRACK.store(track_id, Ordering::SeqCst);
    // Per-track translation toggle (owner 2026-07-23): a new track always
    // starts original-only.
    set_translation_session(false, None);
    publish_loading();
    let title = display_title(track);
    let artist = track.artist.clone();
    ui(move |mut b| {
        b.as_mut().set_track_title(QString::from(title.as_str()));
        b.as_mut().set_track_artist(QString::from(artist.as_str()));
        b.as_mut().set_notice(QString::default());
    });

    let Some(service) = service(runtime).await else {
        publish(
            LyricsDocJson {
                status: 4,
                error: "lyrics service unavailable".to_string(),
                ..Default::default()
            },
            false,
            false,
        );
        return;
    };
    let result = service.get(make_request(track, None)).await;
    // Stale guard (F2): a response for a track that is no longer current is
    // dropped whole.
    if CURRENT_TRACK.load(Ordering::SeqCst) != track_id {
        return;
    }
    commit(result, false);
}

// ---------------------------------------------------------------------------
// Translation (Qobuz API v10)
// ---------------------------------------------------------------------------

/// Account language (ISO 639-1) from the client session — "Auto"'s first hop.
async fn account_language(runtime: &Runtime) -> Option<String> {
    let client_lock = runtime.core().client();
    let guard = client_lock.read().await;
    guard.as_ref()?.session_language_code().await
}

/// Resolve the effective target: the explicit setting when set; "auto" =
/// account `language_code`, falling back to the UI locale.
async fn resolve_target_language(runtime: &Runtime, pref: &str) -> Option<String> {
    if pref != "auto" {
        return Some(pref.to_string());
    }
    if let Some(lang) = account_language(runtime).await {
        return Some(lang);
    }
    let ui_locale = qbz_i18n::current_language();
    (!ui_locale.is_empty()).then(|| ui_locale.to_string())
}

/// Toggle ON. Fails SOFT everywhere: any gap leaves the current lyrics view
/// untouched and reports through the panel-local inline notice.
async fn enable_translation(pref: String) {
    let unavailable = || {
        set_translation_session(false, None);
        publish_notice(qbz_i18n::t(
            "Translation is not available right now. Please try again later.",
        ));
    };
    let Some(runtime) = runtime_handle() else {
        unavailable();
        return;
    };
    let Some(track) = runtime.core().current_track().await else {
        unavailable();
        return;
    };
    if CURRENT_TRACK.load(Ordering::SeqCst) != track.id as i64 {
        unavailable();
        return;
    }
    let Some(target) = resolve_target_language(&runtime, &pref).await else {
        unavailable();
        return;
    };

    // Fast path: the held document already carries this translation (toggle
    // off -> on on the same track) — flip locally, no refetch.
    let already = with_current_doc(|doc| {
        doc.and_then(|d| d.translation.as_ref())
            .and_then(|t| t.lang.as_deref())
            == Some(target.as_str())
    });
    if already {
        set_translation_session(true, Some(target));
        ui(|mut b| {
            b.as_mut().set_show_translation(true);
            b.as_mut().set_translation_busy(false);
        });
        return;
    }

    let Some(service) = service(&runtime).await else {
        unavailable();
        return;
    };
    let track_id = track.id as i64;
    let result = service
        .get(make_request(&track, Some(target.clone())))
        .await;
    // Same stale guard as the track-change path.
    if CURRENT_TRACK.load(Ordering::SeqCst) != track_id {
        return;
    }
    let has_translation = match &result {
        Ok(response) => match &response.outcome {
            LyricsOutcome::Found(found) => found.doc.translation.is_some(),
            _ => false,
        },
        Err(_) => false,
    };
    match result {
        Ok(response) if matches!(response.outcome, LyricsOutcome::Found(_)) => {
            if has_translation {
                set_translation_session(true, Some(target));
            }
            commit(Ok(response), has_translation);
            if !has_translation {
                // The track lacks the target language — the original stays
                // committed above; notice + revert (spec §B.4).
                unavailable();
            }
        }
        // NotFound / offline-miss / transport error: keep the current view.
        Ok(_) => unavailable(),
        Err(e) => {
            log::warn!("[qbz-qt] translation refetch failed: {e}");
            unavailable();
        }
    }
}
