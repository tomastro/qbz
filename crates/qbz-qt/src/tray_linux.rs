//! Linux system tray via `ksni` (StatusNotifierItem) — the ksni half.
//!
//! Port of `crates/qbz/src/tray/linux.rs` (485 L) per the 2026-08-03
//! miniplayer/tray contract (row A-20, §5.2-§5.6). The icon decoding, theme
//! resolution, tooltip composition and the updater-thread pattern are
//! byte-for-byte equivalent; the ONLY behavioural difference is that tray
//! actions call `tray_qt`'s free verbs, which emit `QbzTray` signals, because
//! Rust cannot reach a `QQuickWindow` (§2.3, §5.8).
//!
//! **Mounted `#[cfg(target_os = "linux")]` from `main.rs`** (owner ruling K2 —
//! macOS and Windows are out; §13-D6).
//!
//! ## Why `QSystemTrayIcon` and `Qt.labs.platform.SystemTrayIcon` are not here
//!
//! The process is a `QGuiApplication` (`src/main.rs`), cxx-qt-lib 0.7.3 ships
//! no Widgets bindings at all, and `Qt.labs.platform`'s item has **no scroll
//! signal**, ONE flat `tooltip` string where this tray composes a 3-4 line
//! block, ONE `icon.source` where this tray publishes four pixmaps, and it
//! links QtWidgets — under `QGuiApplication` it silently does not appear
//! (contract §3.3). A hybrid is impossible: the SNI menu is part of the same
//! D-Bus object and `ksni` owns `fn menu()`.

use std::sync::{
    mpsc::{self, Sender},
    Arc, Mutex,
};

use image::GenericImageView;
use ksni::{blocking::TrayMethods, menu::StandardItem, Icon, MenuItem, Orientation, ToolTip, Tray};

use crate::tray_qt::NowPlaying;

// Multiple pixmap sizes per StatusNotifierItem spec — panels pick the best
// match for their bar height (22 = base, 32/44/64 = HiDPI). THE PANEL CHOOSES;
// QBZ publishes all four and does not pick.
//
// Legacy filename note (shared with the Tauri assets): `tray-light-*` holds
// the BLACK glyph (for LIGHT panels) and `tray-dark-*` holds the WHITE glyph.
// The constants use glyph-colour names so the mapping is explicit.
//
// The trap is measured, not merely comment-asserted: `tray-light-22`'s
// opaque-pixel mean RGB is 0.0 and `tray-dark-22`'s is 255.0 — UT-5 asserts on
// exactly that, so no future edit can quietly invert it (§15 trap 18).
//
// The twelve PNGs are COPIED into `crates/qbz-qt/icons/` rather than
// `include_bytes!`d across crates (§13-D17): a Qt-crate -> Slint-binary-crate
// file dependency breaks the day that crate is pruned. They are NOT in
// `qml/assets/` — `build.rs` sweeps that tree into the qrc and these bytes are
// read by Rust, not by QML.
const TRAY_ICON_MONO_BLACK_22: &[u8] = include_bytes!("../icons/tray-light-22.png");
const TRAY_ICON_MONO_BLACK_32: &[u8] = include_bytes!("../icons/tray-light-32.png");
const TRAY_ICON_MONO_BLACK_44: &[u8] = include_bytes!("../icons/tray-light-44.png");
const TRAY_ICON_MONO_BLACK_64: &[u8] = include_bytes!("../icons/tray-light-64.png");
const TRAY_ICON_MONO_WHITE_22: &[u8] = include_bytes!("../icons/tray-dark-22.png");
const TRAY_ICON_MONO_WHITE_32: &[u8] = include_bytes!("../icons/tray-dark-32.png");
const TRAY_ICON_MONO_WHITE_44: &[u8] = include_bytes!("../icons/tray-dark-44.png");
const TRAY_ICON_MONO_WHITE_64: &[u8] = include_bytes!("../icons/tray-dark-64.png");
const TRAY_ICON_COLOR_22: &[u8] = include_bytes!("../icons/tray-color-22.png");
const TRAY_ICON_COLOR_32: &[u8] = include_bytes!("../icons/tray-color-32.png");
const TRAY_ICON_COLOR_44: &[u8] = include_bytes!("../icons/tray-color-44.png");
const TRAY_ICON_COLOR_64: &[u8] = include_bytes!("../icons/tray-color-64.png");

fn is_flatpak() -> bool {
    std::env::var("FLATPAK_ID").is_ok() || std::path::Path::new("/.flatpak-info").exists()
}

/// Detect whether the system prefers a dark color scheme. Tries (in order):
/// GNOME `color-scheme`, GTK `prefer-dark-theme`, KDE `ColorScheme`. Defaults
/// to `false` (light) when nothing matches.
///
/// **The three steps do NOT have the same early-return shape, and that is
/// deliberate** (`crates/qbz/src/tray/linux.rs:46-81`):
///
///  - step 1 can only ever return `true` — a `"default"` / `"prefer-light"`
///    answer falls through;
///  - steps 2 and 3 return on the first KEY MATCH, **falsey value included**, so
///    `gtk-application-prefer-dark-theme=0` returns `false` and never consults
///    gtk-3.0 or kdeglobals. Only the key's ABSENCE falls through.
///
/// **One-shot** — it runs inside `decode_tray_icons`, i.e. at init and on an
/// explicit `SetIconTheme`; there is no live listener for a system light/dark
/// switch. REPRODUCE-1:1 (§12-P10, ticket T-8).
fn prefer_dark_tray() -> bool {
    if let Ok(out) = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        if String::from_utf8_lossy(&out.stdout).contains("prefer-dark") {
            return true;
        }
    }
    if let Some(config) = dirs::config_dir() {
        for variant in ["gtk-4.0", "gtk-3.0"] {
            let path = config.join(variant).join("settings.ini");
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Some(rest) = line
                        .trim()
                        .strip_prefix("gtk-application-prefer-dark-theme")
                    {
                        let v = rest.trim_start_matches(['=', ' ']);
                        return v.starts_with('1') || v.starts_with("true");
                    }
                }
            }
        }
        if let Ok(content) = std::fs::read_to_string(config.join("kdeglobals")) {
            for line in content.lines() {
                if let Some(rest) = line.trim().strip_prefix("ColorScheme") {
                    return rest
                        .trim_start_matches(['=', ' '])
                        .to_lowercase()
                        .contains("dark");
                }
            }
        }
    }
    false
}

/// Convert an embedded RGBA PNG to the ARGB32 big-endian layout ksni expects.
fn decode_pixmap(bytes: &[u8]) -> Result<Icon, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode tray icon: {e}"))?;
    let (width, height) = img.dimensions();
    let mut data = img.into_rgba8().into_vec();
    // ksni spec: ARGB32 with A, R, G, B order per pixel. `image` gives us
    // RGBA; rotate_right(1) moves the alpha byte from the last slot to the
    // first. It is a byte permutation, not a colour conversion: no
    // premultiplication and no further endian swap.
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width: width as i32,
        height: height as i32,
        data,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconVariant {
    /// Black glyph — for light panels.
    MonoBlack,
    /// White glyph — for dark panels (Plasma dark, GNOME top bar).
    MonoWhite,
    /// Full color vinyl.
    Color,
}

/// Resolve which icon variant to load. `theme_override`:
///   - "auto" (or unrecognised) — system color-scheme detection
///   - "mono-light" — white (light-coloured) glyph
///   - "mono-dark"  — black (dark-coloured) glyph
///   - "color"      — full vinyl logo
///
/// **The SECOND inversion, independent of the filenames:** the setting string
/// names the GLYPH's lightness, where the filename names the PANEL. So
/// `"mono-light"` -> `MonoWhite` -> `tray-dark-*.png`. Any port that re-derives
/// a path from the setting string gets this backwards and the icon is invisible
/// on half the desktops (§5.2, §15 trap 18).
fn resolve_variant(theme_override: Option<&str>) -> IconVariant {
    match theme_override {
        Some("mono-light") => IconVariant::MonoWhite,
        Some("mono-dark") => IconVariant::MonoBlack,
        Some("color") => IconVariant::Color,
        _ => {
            if prefer_dark_tray() {
                IconVariant::MonoWhite
            } else {
                IconVariant::MonoBlack
            }
        }
    }
}

/// The four PNG byte sets for a variant, ascending (22/32/44/64) — the order
/// panels see. Split out of `decode_tray_icons` so UT-5 can assert on the BYTES
/// rather than on a filename string, which is what makes the §5.2 filename trap
/// untestable-by-accident.
fn icon_sources(variant: IconVariant) -> [&'static [u8]; 4] {
    match variant {
        IconVariant::MonoBlack => [
            TRAY_ICON_MONO_BLACK_22,
            TRAY_ICON_MONO_BLACK_32,
            TRAY_ICON_MONO_BLACK_44,
            TRAY_ICON_MONO_BLACK_64,
        ],
        IconVariant::MonoWhite => [
            TRAY_ICON_MONO_WHITE_22,
            TRAY_ICON_MONO_WHITE_32,
            TRAY_ICON_MONO_WHITE_44,
            TRAY_ICON_MONO_WHITE_64,
        ],
        IconVariant::Color => [
            TRAY_ICON_COLOR_22,
            TRAY_ICON_COLOR_32,
            TRAY_ICON_COLOR_44,
            TRAY_ICON_COLOR_64,
        ],
    }
}

/// Decode pixmaps (22/32/44/64) for the resolved variant. `collect()` into a
/// `Result` short-circuits on the first decode error, and that error aborts
/// tray creation entirely (§5.6).
fn decode_tray_icons(theme_override: Option<&str>) -> Result<Vec<Icon>, String> {
    icon_sources(resolve_variant(theme_override))
        .iter()
        .map(|b| decode_pixmap(b))
        .collect()
}

/// The ksni item itself.
///
/// It carries no runtime, no window handle and no tokio handle — where the
/// reference needs all three (`tray/linux.rs:164-171`), the Qt verbs are free
/// functions that reach `crate::app()` / `crate::spawn` themselves and emit
/// signals for anything window-shaped. Named `QbzTrayItem` so it cannot be
/// confused with the `QbzTray` QObject in `tray_bridge.rs`.
struct QbzTrayItem {
    icons: Vec<Icon>,
    now_playing: Option<NowPlaying>,
    is_playing: bool,
}

impl Tray for QbzTrayItem {
    fn id(&self) -> String {
        "com.blitzfc.qbz".into()
    }

    fn title(&self) -> String {
        // Untranslated on purpose — 1:1 with `tray/linux.rs:184-186`.
        "QBZ".into()
    }

    fn icon_name(&self) -> String {
        // Intentionally empty: SNI panels (KDE Plasma especially) prefer
        // IconName over IconPixmap when both are present, and resolving the
        // app id against the icon theme picks up the full colour app icon
        // instead of our themed monochrome glyph (issue #362). An empty name
        // forces panels to render IconPixmap directly. DO NOT "improve" this
        // by returning the app id — that IS the regression (§15 trap 19).
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        // The composition lives in `tray_qt` so it is unit-testable without a
        // D-Bus connection (UT-4).
        let (title, description) =
            crate::tray_qt::compose_tooltip(self.now_playing.as_ref(), self.is_playing);
        ToolTip {
            title,
            description,
            icon_name: String::new(),
            icon_pixmap: vec![],
        }
    }

    /// Primary click (left) — toggle main window visibility. Mini-aware: while
    /// the miniplayer is open this raises the MINI (§5.8.2, §13-D21).
    fn activate(&mut self, _x: i32, _y: i32) {
        log::info!("[tray] primary activate (left click)");
        crate::tray_qt::toggle_window();
    }

    /// Secondary click (middle) — play/pause.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        log::info!("[tray] secondary activate (middle click) -> play/pause");
        crate::tray_qt::dispatch_play_pause();
    }

    /// Mouse wheel — adjust volume in 5%-per-notch steps. Panels report ±120
    /// per notch; touch-pad scrolls produce smaller deltas, so we normalise by
    /// 120 and fall back to `signum()`. Horizontal scroll is ignored.
    ///
    /// The 5 % step itself lives in `dispatch_volume_delta`, not here — same
    /// split as the reference.
    fn scroll(&mut self, delta: i32, orientation: Orientation) {
        if !matches!(orientation, Orientation::Vertical) || delta == 0 {
            return;
        }
        let mut ticks = delta / 120;
        if ticks == 0 {
            ticks = delta.signum();
        }
        log::debug!("[tray] scroll delta={} ticks={}", delta, ticks);
        // Positive ticks = wheel-up = volume up.
        crate::tray_qt::dispatch_volume_delta(ticks);
    }

    /// Six items, in this order: Play/Pause · Next · Previous · separator ·
    /// Show/Hide Window · separator · Quit QBZ. **Every item is unconditionally
    /// enabled and unconditionally visible** — there is no enable or visibility
    /// rule anywhere in the reference's builder, and there are no radio,
    /// checkbox or submenu items (§5.3). Note Next comes BEFORE Previous.
    ///
    /// `ksni` calls `menu()` on every update, so the labels re-translate on the
    /// next tray update after a language change — but nothing fires an explicit
    /// refresh on the language edge. REPRODUCE-1:1 (§12-P14, ticket T-10).
    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: qbz_i18n::t("Play/Pause"),
                activate: Box::new(|_this: &mut Self| crate::tray_qt::dispatch_play_pause()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: qbz_i18n::t("Next Track"),
                activate: Box::new(|_this: &mut Self| crate::tray_qt::dispatch_next()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: qbz_i18n::t("Previous Track"),
                activate: Box::new(|_this: &mut Self| crate::tray_qt::dispatch_previous()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: qbz_i18n::t("Show/Hide Window"),
                activate: Box::new(|_this: &mut Self| crate::tray_qt::toggle_window()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: qbz_i18n::t("Quit QBZ"),
                activate: Box::new(|_this: &mut Self| crate::tray_qt::quit()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Updates dispatched from the rest of the app into the ksni worker.
///
/// We cannot call `ksni::blocking::Handle::update` directly from a tokio task —
/// `ksni` 0.3 (`feature = "blocking"`) wraps every update in
/// `Runtime::block_on` (`ksni-0.3.4/src/blocking.rs:206-208`), which panics
/// inside an existing tokio runtime (the playback poll loop runs on one). **The
/// panic is swallowed, so updates appear to succeed but never apply.** We
/// serialise updates over a `std::sync::mpsc` channel applied from a dedicated
/// `std::thread` outside any tokio context. §15 trap 25 — this is structural,
/// not stylistic.
enum TrayUpdate {
    SetTrack {
        title: String,
        artist: String,
        album: String,
    },
    ClearTrack,
    SetPlaying(bool),
    /// Re-decode pixmaps for the requested theme override and push them live
    /// (`NewIcon` SNI signal — panels re-fetch without restart).
    SetIconTheme(String),
}

/// Cross-thread handle to the live ksni tray. Cloneable; mutators forward to
/// the worker thread, safe from any async context. When the tray failed to
/// start the inner sender stays `None` and every mutator is a no-op.
#[derive(Clone)]
pub(crate) struct LinuxTrayHandle {
    sender: Arc<Mutex<Option<Sender<TrayUpdate>>>>,
}

impl LinuxTrayHandle {
    fn empty() -> Self {
        Self {
            sender: Arc::new(Mutex::new(None)),
        }
    }

    /// Own `handle` on a dedicated `std::thread` and drain the channel from
    /// there. The loop ends when every sender has dropped.
    ///
    /// `Handle::update` returns `Option<R>` (`None` once the service is gone),
    /// which is `#[must_use]`; every call below discards it with `let _ =`
    /// because a dead service is exactly the no-op the tray wants.
    fn install(&self, handle: ksni::blocking::Handle<QbzTrayItem>) {
        let (tx, rx) = mpsc::channel::<TrayUpdate>();
        std::thread::Builder::new()
            .name("qbz-tray-updater".into())
            .spawn(move || {
                while let Ok(msg) = rx.recv() {
                    match msg {
                        TrayUpdate::SetTrack {
                            title,
                            artist,
                            album,
                        } => {
                            log::debug!(
                                "[tray] tooltip update -> {} / {} / {}",
                                title,
                                artist,
                                album
                            );
                            let _ = handle.update(move |tray| {
                                tray.now_playing = Some(NowPlaying {
                                    title,
                                    artist,
                                    album,
                                });
                            });
                        }
                        TrayUpdate::ClearTrack => {
                            log::debug!("[tray] tooltip cleared");
                            let _ = handle.update(|tray| {
                                // Clearing the track also drops the play state,
                                // so the middle-click hint cannot go stale.
                                tray.now_playing = None;
                                tray.is_playing = false;
                            });
                        }
                        TrayUpdate::SetPlaying(is_playing) => {
                            let _ = handle.update(move |tray| {
                                tray.is_playing = is_playing;
                            });
                        }
                        TrayUpdate::SetIconTheme(theme) => {
                            log::info!("[tray] icon theme override -> {}", theme);
                            // Decoded HERE, on the updater thread, off the ksni
                            // internals. A decode failure logs and leaves the
                            // OLD icons in place rather than clearing them.
                            match decode_tray_icons(Some(&theme)) {
                                Ok(new_icons) => {
                                    let _ = handle.update(move |tray| {
                                        tray.icons = new_icons;
                                    });
                                }
                                Err(e) => {
                                    log::error!(
                                        "[tray] failed to decode icons for theme '{}': {}",
                                        theme,
                                        e
                                    );
                                }
                            }
                        }
                    }
                }
                log::debug!("[tray] updater thread exiting");
            })
            .expect("spawn tray updater thread");
        if let Ok(mut guard) = self.sender.lock() {
            *guard = Some(tx);
        }
    }

    fn send(&self, msg: TrayUpdate) {
        let guard = match self.sender.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(msg);
        }
    }

    pub(crate) fn set_track(&self, title: String, artist: String, album: String) {
        self.send(TrayUpdate::SetTrack {
            title,
            artist,
            album,
        });
    }

    pub(crate) fn clear_track(&self) {
        self.send(TrayUpdate::ClearTrack);
    }

    pub(crate) fn set_playing(&self, is_playing: bool) {
        self.send(TrayUpdate::SetPlaying(is_playing));
    }

    pub(crate) fn set_icon_theme(&self, theme: String) {
        self.send(TrayUpdate::SetIconTheme(theme));
    }
}

/// Initialize the Linux ksni tray service. Spawns a background thread that owns
/// the SNI service and returns a cloneable handle for live tooltip / theme
/// updates. `theme_override`: "auto"/"mono-light"/"mono-dark"/"color".
///
/// **Called ONLY from the `qbz-tray-init` thread** (`tray_qt::init`): `spawn()`
/// below is the blocking API and panics inside a tokio runtime.
pub(crate) fn init(theme_override: &str) -> Result<LinuxTrayHandle, Box<dyn std::error::Error>> {
    log::info!(
        "Initializing ksni tray (Linux, SNI primary-activate enabled, theme={:?})",
        theme_override
    );

    // An icon-decode Err aborts tray creation entirely and propagates to the
    // init thread's Err arm, which leaves `tray_qt::handle()` at None and
    // pushes `tray_live = false` — so close-to-tray quits instead of hiding
    // (AC-32).
    let icons = decode_tray_icons(Some(theme_override))?;
    let tray = QbzTrayItem {
        icons,
        now_playing: None,
        is_playing: false,
    };

    // Flatpak requires disabling the well-known DBus name because the sandbox
    // cannot own arbitrary bus names (§15 trap 21).
    let ksni_handle = if is_flatpak() {
        log::info!("[tray] Flatpak detected — spawning ksni without DBus well-known name");
        tray.disable_dbus_name(true).spawn()?
    } else {
        tray.spawn()?
    };

    let live = LinuxTrayHandle::empty();
    live.install(ksni_handle);

    log::info!("ksni tray initialized");
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean RGB over the OPAQUE pixels of a decoded pixmap. The data is ARGB32
    /// after `decode_pixmap`, so byte 0 of each pixel is alpha and bytes 1..4
    /// are R, G, B.
    fn opaque_mean_rgb(bytes: &[u8]) -> f64 {
        let icon = decode_pixmap(bytes).expect("decode");
        let mut sum = 0u64;
        let mut count = 0u64;
        for px in icon.data.chunks_exact(4) {
            if px[0] == 0 {
                continue;
            }
            sum += px[1] as u64 + px[2] as u64 + px[3] as u64;
            count += 3;
        }
        assert!(count > 0, "no opaque pixels");
        sum as f64 / count as f64
    }

    /// UT-5 — variant resolution and THE FILENAME INVERSION (AC-21).
    ///
    /// Asserted on the BYTES, never on a filename string: `"mono-light"` must
    /// end up loading a WHITE glyph, whatever the file is called. Getting this
    /// backwards makes the icon invisible on half the desktops, and a
    /// name-based assertion would happily pass while it was wrong.
    #[test]
    fn tray_icon_variant_resolution() {
        // "mono-light" -> MonoWhite -> tray-dark-*.png -> a WHITE glyph.
        assert_eq!(resolve_variant(Some("mono-light")), IconVariant::MonoWhite);
        let white = icon_sources(IconVariant::MonoWhite);
        assert_eq!(white[0], TRAY_ICON_MONO_WHITE_22);
        assert!(
            (opaque_mean_rgb(white[0]) - 255.0).abs() < 1.0,
            "mono-light must render a WHITE glyph (mean {})",
            opaque_mean_rgb(white[0])
        );

        // "mono-dark" -> MonoBlack -> tray-light-*.png -> a BLACK glyph.
        assert_eq!(resolve_variant(Some("mono-dark")), IconVariant::MonoBlack);
        let black = icon_sources(IconVariant::MonoBlack);
        assert_eq!(black[0], TRAY_ICON_MONO_BLACK_22);
        assert!(
            opaque_mean_rgb(black[0]) < 1.0,
            "mono-dark must render a BLACK glyph (mean {})",
            opaque_mean_rgb(black[0])
        );

        // The two mono sets are genuinely different byte sets, in all 4 sizes.
        for i in 0..4 {
            assert_ne!(white[i], black[i], "size index {i}");
        }

        // "color" -> the vinyl set, which is neither of the monos.
        assert_eq!(resolve_variant(Some("color")), IconVariant::Color);
        let color = icon_sources(IconVariant::Color);
        assert_eq!(color[0], TRAY_ICON_COLOR_22);
        let color_mean = opaque_mean_rgb(color[0]);
        assert!(
            color_mean > 1.0 && color_mean < 254.0,
            "color is not a monochrome glyph (mean {color_mean})"
        );

        // All four sizes are published, ascending, and each decodes square at
        // its nominal size (AC-22).
        for (bytes, expected) in color.iter().zip([22, 32, 44, 64]) {
            let icon = decode_pixmap(bytes).expect("decode");
            assert_eq!(icon.width, expected);
            assert_eq!(icon.height, expected);
            assert_eq!(icon.data.len() as i32, expected * expected * 4);
        }

        // "auto", an unknown string and None all take the prefer_dark_tray
        // branch — the same branch, not three different ones.
        //
        // Asserted WITHOUT re-deriving the expected value from
        // `prefer_dark_tray()`: computing the expectation with the same
        // expression the code under test uses makes the test tautological —
        // inverting that arm would flip both sides and it would still pass.
        // Two properties are checked instead, and both survive an inversion:
        // the three spellings agree with each other, and the result is one of
        // the two MONO variants (never Color, which only an explicit "color"
        // may produce).
        let auto = resolve_variant(Some("auto"));
        assert_eq!(resolve_variant(Some("banana")), auto);
        assert_eq!(resolve_variant(None), auto);
        assert!(
            matches!(auto, IconVariant::MonoWhite | IconVariant::MonoBlack),
            "the auto ladder must resolve to a mono glyph, got {auto:?}"
        );
        assert_ne!(resolve_variant(Some("color")), auto);
    }
}
