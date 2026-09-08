//! QbzMini — miniplayer domain bridge (the immersive_bridge.rs exemplar:
//! `OnceLock<CxxQtThread>`, `ui()` no-op pre-boot, `boot()` invokable, `impl
//! Default` for construction-seeded values, a custom-WRITE property funnel with
//! an `AtomicBool` mirror beside it).
//!
//! Port of the Slint `MiniPlayerState` global per the 2026-08-03
//! miniplayer/tray contract (`qbz-nix-docs/qt-frontend/2026-08-03-miniplayer-tray-port/00-CONTRACT.md`,
//! rows A-1 … A-9). The controller half — the queue projection, the prefs and
//! the pure arithmetic — is `mini_qt.rs`.
//!
//! **There is no mirror layer and there must never be one** (contract §3.2 /
//! §13-D1). Slint needs ~155 lines of `d.set_x(s.get_x())` because its globals
//! are per-component-tree; in Qt every bridge is a `#[qml_singleton]` in the ONE
//! `QQmlApplicationEngine`, so a second `Window` reads the same `QbzPlayer` /
//! `QbzQueue` / `QbzLyrics` / `QbzShell` objects the shell reads.
//!
//! `surface` and `open` are CUSTOM WRITES. QML reaches them by assignment
//! (`QbzMini.surface = 3`), and the funnel — not the call site — owns the
//! persist, the `OPEN` mirror and the surface-3 queue refresh, so no write path
//! can skip them.

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_mini {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // Miniplayer-mode flag. CUSTOM WRITE (the `set_open` funnel below), so
        // every write — QML's exit paths and the Rust verbs alike — passes
        // through one setter that also stores the `OPEN` mirror (contract A-2,
        // the exact shape of immersive_bridge.rs's `open`).
        #[qproperty(bool, open, READ, WRITE = set_open, NOTIFY)]
        // 0 micro · 1 compact · 2 artwork · 3 queue · 4 lyrics — the
        // MINI_VIEW_VALUES order minus "remember" (settings_qt.rs:1077).
        // CUSTOM WRITE, and it is the ONLY compilable reading of contract A-9:
        // that row asks for `#[qinvokable] fn set_surface`, but A-3 fixes the
        // property name `surface`, whose cxx-qt-generated setter is ALSO
        // `set_surface`/`setSurface` — one QObject cannot carry both. The Rust
        // signature is byte-identical to A-9's; only the attribute differs, and
        // routing it through the property is strictly stronger because a plain
        // assignment cannot bypass the persist. QML therefore writes
        // `QbzMini.surface = n` and MUST NOT call `QbzMini.setSurface(n)` —
        // that member does not exist on the object.
        #[qproperty(i32, surface, READ, WRITE = set_surface, NOTIFY)]
        // The static artwork-derived card background (`mini_background_blur`).
        // Written by `toggle_background_blur` (contract A-10).
        #[qproperty(bool, background_blur)]
        // The navigable queue: the current track first, then the FULL
        // unfiltered upcoming list. Built by `mini_qt::mini_queue_doc` and
        // published from the ONE queue funnel (`queue_qt::publish`), never from
        // a second fetch (§7-M1). Full-shape default, never "{}" (trap 15).
        #[qproperty(QString, mini_queue_json)]
        // THE PERSISTED EXPANDED GEOMETRY, exposed to QML (contract A-6b).
        // The `Window` is QML-created and Rust cannot reach a QQuickWindow, so
        // QML owns the sizing and needs the numbers; `save_geometry` writes the
        // clamped pair back here, so a condensed->expanded switch inside one
        // session restores the size the user last dragged without a re-read.
        // Seeded at CONSTRUCTION, not at boot(): the size expression is a
        // binding and the first frame is already too late for a post-boot push.
        #[qproperty(f32, mini_width)]
        #[qproperty(f32, mini_height)]
        type QbzMini = super::QbzMiniRust;

        /// Registers this object's Qt-thread hop (Main.qml boots EVERY domain
        /// singleton; only QbzSession.boot also fires crate::on_boot).
        /// **Without it every `mini_bridge::ui()` is a SILENT no-op** — the
        /// bridge never errors, it just never repaints.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzMini>);

        /// Enter the miniplayer (contract A-8, §4.6's `enter()`). Suppressed
        /// under gamescope — the guard lives HERE so the view-menu row and the
        /// `ui.miniPlayer` hotkey share one predicate (§4.10.1).
        #[qinvokable]
        fn enter(self: Pin<&mut QbzMini>);

        /// Leave the miniplayer (contract A-8, §4.6's `exit()`). Also the
        /// destination of the mini's `Escape` / `Shift+M` and of its
        /// `onClosing` — a compositor close behaves like the expand button
        /// (§13-D13).
        #[qinvokable]
        fn exit(self: Pin<&mut QbzMini>);

        /// Flip + persist the card backdrop (contract A-10). The capsule's
        /// `a-bg` button is its only caller; the whole render path already
        /// exists in `MiniShell.qml` gated on this one property.
        #[qinvokable]
        fn toggle_background_blur(self: Pin<&mut QbzMini>);

        /// Persist the expanded window size (contract A-14). Qt has no
        /// `WindowEvent::Resized`, so `MiniWindow.qml`'s 400 ms debounce calls
        /// in. The §4.7 guards live in `mini_qt::geometry_to_persist` — the
        /// surface is read from THIS object rather than passed, so no call site
        /// can disagree with the property the window is sized from.
        #[qinvokable]
        fn save_geometry(self: Pin<&mut QbzMini>, width: f32, height: f32);

        /// Activate a queue-surface row (contract A-13, §4.4.3). `row` indexes
        /// `mini_queue_json.rows` — the DISPLAY list, whose first entry is the
        /// current track only when there IS one.
        ///
        /// Remote-first, and `Err` RETURNS (§15 trap 9); row 0 resumes ONLY
        /// when the document carries a current track (the off-by-one the
        /// reference has and this port fixes); everything else is an index
        /// into the UNFILTERED upcoming list (§15 trap 10).
        #[qinvokable]
        fn queue_play(self: Pin<&mut QbzMini>, row: i32);

        /// Close the whole APP from the mini (contract A-12, §4.6). Exits the
        /// mini first, then routes to the ONE close choreography by emitting
        /// `close_app_requested` below.
        #[qinvokable]
        fn close_app(self: Pin<&mut QbzMini>);

        /// `close_app`'s second half: `Main.qml` answers with
        /// `window.closeOrHide(null)`, so the mini's red X honours
        /// close-to-tray exactly like every other exit.
        ///
        /// **This signal is an ADDITION to the contract's §2.2 must-add list**,
        /// and it is logged as one. A-12 says `close_app` "routes to the ONE
        /// close choreography (§5.7)", but that choreography is a QML function
        /// on `Main.qml`'s root: `QbzMini` had no signals at all, the mini's
        /// QML tree holds no reference to the main window (`MiniWindow.qml`
        /// passes `hostWindow: root`, i.e. the MINI), and none of `QbzTray`'s
        /// four signals is a "close requested" — routing through
        /// `quit_requested` would bypass close-to-tray outright. The only
        /// alternative that adds no symbol is deciding hide-vs-quit in Rust,
        /// which would put a SECOND copy of the close predicate in the tree
        /// (the first is QML's `trayLive && closeToTray`) — the
        /// two-functions-that-should-mirror-each-other smell LESSONS L5 names.
        /// One signal keeps exactly one copy of the predicate.
        #[qsignal]
        fn close_app_requested(self: Pin<&mut QbzMini>);

        /// The MINI window's own key dispatcher (contract A-15, §4.9), based
        /// on the reference's `dispatch_mini`
        /// (`crates/qbz/src/keybindings.rs:623-648`) plus the context-free
        /// playback actions added post-port. **No
        /// `text_input_focused` argument** — the mini has no text fields, and
        /// `dispatch_mini` has no text-input guard either. `true` = consumed.
        #[qinvokable]
        fn key_pressed(self: Pin<&mut QbzMini>, key: i32, modifiers: i32, text: QString) -> bool;
    }

    // The custom WRITE targets. NOT qinvokables — QML reaches them by writing
    // `QbzMini.open` / `QbzMini.surface`, never by name. (cxx-qt custom-setter
    // pattern: the property's generated setter is replaced by this method,
    // which owns the store + notify + the side effects.)
    unsafe extern "RustQt" {
        fn set_open(self: Pin<&mut QbzMini>, value: bool);
        fn set_surface(self: Pin<&mut QbzMini>, surface: i32);
    }

    impl cxx_qt::Threading for QbzMini {}
}

use qbz_mini::QbzMini;

/// Rust side of the mini bridge (plain storage, phase-1 pattern).
pub struct QbzMiniRust {
    open: bool,
    surface: i32,
    background_blur: bool,
    mini_queue_json: QString,
    mini_width: f32,
    mini_height: f32,
}

impl Default for QbzMiniRust {
    fn default() -> Self {
        // Read here, not in boot(): QML singletons instantiate lazily and the
        // window's size + surface bindings evaluate on its first frame, which
        // can precede QbzMini.boot(). A post-boot seed would leave the first
        // frame decided against cxx-qt's derived zeroes (trap 31).
        Self {
            open: false,
            surface: crate::mini_qt::initial_surface(),
            background_blur: crate::mini_qt::initial_background_blur(),
            mini_queue_json: QString::from(crate::mini_qt::MINI_QUEUE_EMPTY),
            mini_width: crate::mini_qt::initial_width(),
            mini_height: crate::mini_qt::initial_height(),
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (immersive_bridge.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzMini>> = OnceLock::new();

/// Queue a mini-bridge mutation onto the Qt event loop (no-op before boot
/// registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzMini>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

/// Rust-side mirror of the `open` property, written inside `apply_open` — the
/// funnel EVERY write passes through — so it is exact. Read by callers that run
/// on ANOTHER singleton or on a worker thread and therefore cannot reach this
/// QObject's properties: the hotkeys pipeline's mini/main context split and the
/// surface-3 queue refresh below (contract A-2).
static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn is_open() -> bool {
    OPEN.load(std::sync::atomic::Ordering::SeqCst)
}

/// The single funnel behind every `open` write: store, mirror, THEN notify.
///
/// The mirror is stored BEFORE the notify on purpose. `open_changed()` emits
/// synchronously, so any QML `onOpenChanged` handler runs inside that call —
/// and anything it reaches that asks `is_open()` (the queue refresh below does)
/// would read the stale value if the store came last. `immersive_bridge.rs`'s
/// `apply_open` has the same two lines in the other order; the window there is
/// currently unreachable, so it is left alone rather than widened into this
/// commit.
fn apply_open(mut this: Pin<&mut QbzMini>, value: bool) {
    use cxx_qt::CxxQtType as _;
    if this.open == value {
        return;
    }
    this.as_mut().rust_mut().open = value;
    OPEN.store(value, std::sync::atomic::Ordering::SeqCst);
    this.as_mut().open_changed();
}

/// The single funnel behind every `surface` write: store, notify, persist, and
/// — on the queue surface — kick the queue funnel.
///
/// The persist and the refresh run on EVERY write, including a same-value one,
/// which is 1:1 with the reference (`miniplayer.rs:302-315` has no equality
/// guard: it saves the pref and re-kicks `spawn_queue_refresh` unconditionally).
/// Only the change NOTIFICATION is suppressed when nothing moved, which is
/// ordinary QML property semantics and not a behaviour the reference has an
/// analogue for. Re-asserting surface 3 therefore still refreshes the queue.
fn apply_surface(mut this: Pin<&mut QbzMini>, surface: i32) {
    use cxx_qt::CxxQtType as _;
    if this.surface != surface {
        this.as_mut().rust_mut().surface = surface;
        this.as_mut().surface_changed();
    }
    crate::mini_qt::save_surface(surface);
    if surface == 3 {
        refresh_queue();
    }
}

/// Port of `miniplayer.rs`'s `spawn_queue_refresh` (`:631-641`): gated on the
/// mini being open, and routed through THE queue funnel — `queue_qt::publish`,
/// the one place that reads queue state — never through a second fetch of its
/// own (§7-M1). The funnel's identical-document guards make it a no-op when
/// nothing moved, so the cost of a redundant call is one build, no Qt hop.
fn refresh_queue() {
    if !is_open() {
        return;
    }
    let runtime = crate::app();
    crate::spawn(async move { crate::queue_qt::publish(&runtime).await });
}

/// `enter()`'s body as a free fn, so the invokable and the `ui()`-hopped
/// `toggle()` below reach the SAME code instead of two copies that can drift.
///
/// The reference's `enter()` (`crates/qbz/src/miniplayer.rs:154-188`) is seven
/// ordered steps; five of them have no Qt counterpart. Creation + seeding is
/// QML's `createObject` plus this bridge's construction-time seeds (steps 1-2),
/// `ensure_lyrics_engine` is DROPPED (§3.2 — the mini mounts its own engine),
/// `force_full_mirror` and `spawn_queue_refresh` are DROPPED because the
/// singletons the mini reads are the ones the shell reads and are already
/// current. What remains is the flag — and QML's `onOpenChanged` does the rest.
///
/// Step 7, `m.hide()` LAST, is `window.hideToTray()` (A-25) and lands with B6;
/// until then the main window stays visible behind the mini. Open-coding a bare
/// `window.hide()` here would create a second visibility owner, which §5.8.1's
/// invariant and stage-0 S0-4 forbid.
fn enter_now(mut this: Pin<&mut QbzMini>) {
    if crate::tray_qt::in_gamescope() {
        // 1:1 with the reference's two suppressed entry points
        // (`crates/qbz/src/main.rs:11732-11733`, `keybindings.rs:548-552`): an
        // extra mapped window can steal gamescope's focused-window pick.
        log::info!("[qbz-qt] miniplayer: entry suppressed under gamescope");
        return;
    }
    if this.open {
        // Already up: `apply_open` would early-return on the unchanged value,
        // no `openChanged` would fire, and QML's handler — the ONLY place the
        // window is shown or raised — would never run. The reference's
        // `enter()` re-applies geometry and `show()`s unconditionally
        // (`miniplayer.rs:154-188`), so re-entering RAISES. Forcing the notify
        // is how a Rust-side verb reaches a QML-owned window; it is also the
        // shape `present()` wants.
        this.as_mut().open_changed();
        return;
    }
    apply_open(this, true);
}

/// `exit()`'s body as a free fn — see `enter_now` for why.
///
/// The reference sets `OPEN = false` FIRST so any in-flight work bails
/// (`miniplayer.rs:191-215`); `apply_open` stores the mirror before it notifies,
/// so that ordering is preserved. Restoring the force-opened lyrics panel is
/// DROPPED with the hack that needed it (§3.2). `m.show()` is
/// `window.showFromTray()` (A-25) and lands with B6, for the same
/// one-visibility-owner reason `enter_now` states.
fn exit_now(this: Pin<&mut QbzMini>) {
    apply_open(this, false);
}

/// The `ui.miniPlayer` hotkey's verb — the reference's `toggle_miniplayer()`
/// (`crates/qbz/src/keybindings.rs:547-558`) minus its gamescope guard, which
/// lives in `enter_now` so both entry points share ONE predicate (§4.10.1).
///
/// Hopped through `ui()` because the caller is `hotkeys_bridge`'s `run_action`,
/// which runs on ANOTHER QObject and cannot reach this one's properties — the
/// same reason the `OPEN` mirror exists.
pub(crate) fn toggle() {
    ui(|mini| {
        if is_open() {
            exit_now(mini);
        } else {
            enter_now(mini);
        }
    });
}

impl qbz_mini::QbzMini {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] mini Qt thread already registered");
        }
    }

    /// The custom WRITE target of the `open` property.
    pub fn set_open(self: Pin<&mut Self>, value: bool) {
        apply_open(self, value);
    }

    /// The custom WRITE target of the `surface` property (contract A-9):
    /// sets the property, persists `mini_surface`, and refreshes the queue on
    /// surface 3.
    pub fn set_surface(self: Pin<&mut Self>, surface: i32) {
        apply_surface(self, surface);
    }

    /// Contract A-8 — enter.
    pub fn enter(self: Pin<&mut Self>) {
        enter_now(self);
    }

    /// Contract A-8 — exit.
    pub fn exit(self: Pin<&mut Self>) {
        exit_now(self);
    }

    /// Contract A-10 — the capsule's `a-bg` toggle.
    ///
    /// Read-flip-write through the GENERATED setter, so the change
    /// notification QML's backdrop gate depends on is emitted for free
    /// (`miniplayer.rs:328-330` does the same two steps).
    pub fn toggle_background_blur(mut self: Pin<&mut Self>) {
        let value = !self.background_blur;
        self.as_mut().set_background_blur(value);
        crate::mini_qt::save_background_blur(value);
    }

    /// Contract A-14 — the debounced resize persist.
    ///
    /// Three things happen here and the ORDER matters. The §4.7 guards decide
    /// first (`surface >= 2` AND `height >= 320`, two different numbers from
    /// the 420 floor — §15 trap 17); then a dirty check drops the write when
    /// the clamped pair already matches what this object holds, which is what
    /// makes the write-back below safe; then the clamped pair goes to BOTH the
    /// A-6b properties and the prefs document.
    ///
    /// The write-back is A-14's own requirement: the window is sized from
    /// `miniWidth`/`miniHeight`, so a condensed -> expanded switch inside one
    /// session restores the size the user last dragged without re-reading the
    /// file. It also closes the loop it appears to open — the properties feed
    /// the window's size binding, which feeds the debounce, which calls back in
    /// here with the same numbers and stops at the dirty check.
    ///
    /// The reference has NO dirty check and does a whole-file load+save per
    /// qualifying resize event (`miniplayer.rs:455-463`) — §13-D14 — where its
    /// own main window carries one for exactly this reason
    /// (`crates/qbz/src/main.rs:1392-1393`).
    pub fn save_geometry(mut self: Pin<&mut Self>, width: f32, height: f32) {
        let Some((w, h)) = crate::mini_qt::geometry_to_persist(self.surface, width, height) else {
            return;
        };
        // Sub-pixel equality, the same shape as the main window's `> 0.5`
        // guard: the WM emits many no-op geometry events and each miss here is
        // a blocking JSON read+write on the GUI thread (§7-M10).
        if (self.mini_width - w).abs() < 0.5 && (self.mini_height - h).abs() < 0.5 {
            return;
        }
        self.as_mut().set_mini_width(w);
        self.as_mut().set_mini_height(h);
        crate::mini_qt::save_geometry(w, h);
    }

    /// Contract A-13 — a queue-surface row was clicked.
    ///
    /// Both numbers this needs come out of the document THIS object already
    /// holds: no second `get_queue_state_full()`, no second poll (§7-M1). The
    /// resolution is deliberately synchronous and on the Qt thread — it is a
    /// JSON read of a string the bridge owns, and the reference resolves its
    /// own row on the UI thread for the same reason (`miniplayer.rs:345-363`).
    ///
    /// THE ORDER, which is the whole of §4.4.3 and §8.1's third row:
    ///
    /// 1. `currentId != ""` decides whether row 0 IS the current track. The
    ///    reference assumes it always is (`miniplayer.rs:379-388`) while its
    ///    own model pushes that row only `if let Some(t)` — so with no current
    ///    track, clicking row 0 merely toggles play there. `queue_row_target`
    ///    is the fix and UT-2 is its lock.
    /// 2. The row's id is resolved and parsed BEFORE anything is dispatched,
    ///    because the remote ladder needs the track id and a bad row must cost
    ///    nothing. A non-numeric id returns silently, 1:1 with `:361-363`.
    /// 3. The subtraction (`row - 1`) happens AFTER the current-track test,
    ///    never before.
    ///
    /// Then, on the runtime: the remote-first ladder, in its three-arm form.
    /// `Ok(true)` the peer took it; `Ok(false)` fall through to local; **`Err`
    /// logs and RETURNS** — an error means the peer's state is unknown and
    /// starting local audio would double-play (§15 trap 9). The idiom is
    /// `queue_qt::reorder_upcoming`'s (`src/queue_qt.rs:679-687`).
    pub fn queue_play(self: Pin<&mut Self>, row: i32) {
        if row < 0 {
            return;
        }
        let doc: serde_json::Value = match serde_json::from_str(&self.mini_queue_json.to_string()) {
            Ok(doc) => doc,
            Err(e) => {
                log::warn!("[qbz-qt] mini: queue document unreadable: {e}");
                return;
            }
        };
        let has_current = doc
            .get("currentId")
            .and_then(|v| v.as_str())
            .is_some_and(|id| !id.is_empty());
        // Bounds check on the DISPLAY list, exactly where the reference makes
        // it (`miniplayer.rs:352-358` checks against `row_count()`).
        let Some(id) = doc
            .get("rows")
            .and_then(|rows| rows.get(row as usize))
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
        else {
            log::warn!("[qbz-qt] mini: queue row {row} out of range");
            return;
        };
        let Ok(track_id) = id.parse::<u64>() else {
            // Silent, 1:1 with the reference: a non-numeric id has no local
            // fallback to take (`miniplayer.rs:361-363`).
            return;
        };
        let target = crate::mini_qt::queue_row_target(has_current, row);
        let runtime = crate::app();
        crate::spawn(async move {
            if let Some(svc) = crate::qconnect_qt::service() {
                match svc.play_remote_renderer_track_if_active(track_id).await {
                    Ok(true) => return, // handled by the peer
                    Ok(false) => {}     // not applicable -> local path
                    Err(e) => {
                        log::warn!("[qbz-qt] mini: queue play remote handoff: {e}");
                        return; // connected but errored -> do NOT play locally
                    }
                }
            }
            match target {
                // "resume if paused (no restart)" — never a seek, never a
                // re-open of the same stream. Clicking the row that is already
                // playing is a no-op.
                crate::mini_qt::MiniRowTarget::Resume => {
                    if !crate::now_playing::playing() {
                        crate::transport_toggle_play();
                    }
                }
                // The UNFILTERED upcoming domain — `play_upcoming_flat`, never
                // the page-local `play_upcoming` (§15 trap 10 / trap 23).
                crate::mini_qt::MiniRowTarget::Upcoming(index) => {
                    crate::queue_qt::play_upcoming_flat(&runtime, index.max(0) as usize).await;
                }
            }
        });
    }

    /// Contract A-12 — close the APP from the mini's red X.
    ///
    /// The order is the reference's (`crates/qbz/src/miniplayer.rs:235-251`):
    /// leave the mini FIRST — which, through `Main.qml`'s `onOpenChanged`,
    /// hides the mini and brings the main window back — and only then ask for
    /// the close, because "the window becomes user-visible when close-to-tray
    /// keeps the app alive" (`:244-246`). `open_changed()` emits synchronously,
    /// so the main window is already up by the time the signal below is queued.
    pub fn close_app(mut self: Pin<&mut Self>) {
        exit_now(self.as_mut());
        self.close_app_requested();
    }

    /// Contract A-15 — the mini's key dispatcher, the table at §4.9.
    ///
    /// Resolution goes through `hotkeys_qt::action_for_mini_key` (A-16), which
    /// reads the user's ACTIVE bindings, so a rebound `mini.queue` works here
    /// exactly as the reference's `dispatch_mini` intends (`keybindings.rs:631`).
    /// A matched action consumes (`:647`); anything else propagates (`:645`).
    pub fn key_pressed(self: Pin<&mut Self>, key: i32, modifiers: i32, text: QString) -> bool {
        let text = text.to_string();
        let overrides = crate::hotkeys_qt::load_overrides();
        let Some(action) =
            crate::hotkeys_qt::action_for_mini_key(
                crate::hotkeys_qt::active_keymap(),
                &overrides,
                key,
                modifiers,
                &text,
            )
        else {
            return false;
        };
        // The surface writes go through `apply_surface`, the same funnel
        // `QbzMini.surface = n` uses, so the keyboard path cannot skip the
        // persist or the surface-3 queue kick.
        //
        // All five arms are UNCLAMPED. The `clamp_to_implemented` guard that
        // used to wrap `3` and `4` existed only while their surfaces were
        // unwritten; B4 landed both and deleted it, so a key now lands exactly
        // where the user's binding says.
        match action.id {
            "mini.micro" => apply_surface(self, 0),
            "mini.compact" => apply_surface(self, 1),
            "mini.artwork" => apply_surface(self, 2),
            "mini.queue" => apply_surface(self, 3),
            "mini.lyrics" => apply_surface(self, 4),
            // Both leave the mini — 1:1 with `keybindings.rs:641`, where the
            // two ids share one arm.
            "ui.miniPlayer" | "ui.escape" => exit_now(self),
            "playback.toggle" => crate::transport_toggle_play(),
            "playback.next" => crate::transport_next(),
            "playback.prev" => crate::transport_previous(),
            // Context::None playback actions stay global in the separate mini
            // window too. Keep these routed through the same helpers as the
            // main dispatcher so direct/exclusive volume locks cannot diverge.
            "playback.volumeUp" => {
                crate::hotkeys_bridge::nudge_volume(crate::hotkeys_bridge::VOLUME_STEP)
            }
            "playback.volumeDown" => {
                crate::hotkeys_bridge::nudge_volume(-crate::hotkeys_bridge::VOLUME_STEP)
            }
            "playback.mute" => crate::hotkeys_bridge::toggle_mute(),
            "playback.shuffle" => crate::transport_toggle_shuffle(),
            "playback.repeat" => crate::transport_cycle_repeat(),
            "playback.favorite" => crate::hotkeys_bridge::request_favorite(),
            "playback.seekForward" => crate::hotkeys_bridge::seek_relative(5),
            "playback.seekBack" => crate::hotkeys_bridge::seek_relative(-5),
            // Navigation and immersive-only actions still belong to the main
            // window; unmatched Context::None rows propagate here.
            _ => return false,
        }
        true
    }
}
