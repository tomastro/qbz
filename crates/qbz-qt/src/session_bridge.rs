//! QbzSession — session/auth/offline/i18n domain bridge (phase 23 split of
//! the QbzBridge God-object; the pattern is documented in main.rs). Props:
//! screen/login/offline/connectivity/recovery/session-user + trRev.
//! Invokables: boot (THE app boot — registers this object's thread hop AND
//! fires crate::on_boot once), the login/logout flow, and `tr` (the one
//! i18n lookup every QML string binds to, with the trRev dependency arg).

use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::CxxQtThread;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qbz_session {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        // "splash" | "login" | "shell" | "kiosk"
        //
        // "kiosk" is the small-panel touch shell — a SIBLING of "shell", not a
        // mode inside it, exactly as the reference models it
        // (`qbz-ui/ui/app.slint:38`: `enum AppScreen { splash, login, shell,
        // kiosk }`, with the two shells as sibling mounts at `:145` and
        // `:205`). Every `*State` global survives the swap untouched, which is
        // what lets the live toggle keep playback running.
        #[qproperty(QString, screen)]
        // Login narration: 0 idle / 1 waiting-for-browser / 2 authenticating.
        #[qproperty(i32, login_phase)]
        // Last SIGN-IN failure message ("" = none). Mirrors Slint
        // `LoginState.error`.
        #[qproperty(QString, login_error)]
        // Session-RESTORE failure message ("" = none). Mirrors Slint
        // `OfflineState.login-error`.
        #[qproperty(QString, restore_error)]
        // Offline-MODE engine snapshot (Slint `OfflineState`):
        #[qproperty(bool, offline)]
        // 0 online / 1 real offline / 2 induced offline.
        #[qproperty(i32, offline_mode)]
        // 0 unknown / 1 up / 2 down.
        #[qproperty(i32, connectivity)]
        #[qproperty(bool, captive_portal)]
        #[qproperty(bool, has_previous_session)]
        #[qproperty(bool, show_recovery_banner)]
        #[qproperty(bool, offline_session)]
        // Shell session header:
        #[qproperty(QString, session_user_name)]
        #[qproperty(QString, session_subscription)]
        // Translation revision (phase 20): bumped on every live language
        // switch; every `QbzSession.tr(msgid, QbzSession.trRev)` binding
        // depends on it.
        #[qproperty(i32, tr_rev)]
        type QbzSession = super::QbzSessionRust;

        /// The first invokable (Main.qml Component.onCompleted): register
        /// this object's Qt-thread hop, then run the app boot sequence.
        #[qinvokable]
        fn boot(self: Pin<&mut QbzSession>);
        /// Start the Qobuz browser-OAuth login flow.
        #[qinvokable]
        fn sign_in_via_browser(self: Pin<&mut QbzSession>);
        /// Abort the in-flight login (back to idle).
        #[qinvokable]
        fn cancel_login(self: Pin<&mut QbzSession>);
        /// Continue in induced-offline mode (no Qobuz services).
        #[qinvokable]
        fn start_offline(self: Pin<&mut QbzSession>);
        /// Retry session restore after a failure.
        #[qinvokable]
        fn recovery_login(self: Pin<&mut QbzSession>);
        /// Open the Qobuz Terms of Service in the browser.
        #[qinvokable]
        fn open_tos(self: Pin<&mut QbzSession>);
        /// Shell logout: back to the login screen.
        #[qinvokable]
        fn logout(self: Pin<&mut QbzSession>);
        /// Kiosk <-> Desktop live toggle (2026-08-02 kiosk-port contract
        /// §8.1). Persists the profile, flips `QbzShell.kioskProfile` and
        /// swaps `screen` between "kiosk" and "shell" — the shells share every
        /// bridge, so nothing is torn down and playback keeps running.
        #[qinvokable]
        fn toggle_profile(self: Pin<&mut QbzSession>);

        /// i18n lookup against the shared gettext catalog (qbz-i18n), so the
        /// QML texts reuse the EXISTING .po translations via the same msgids
        /// the Slint `@tr()` calls use. `rev` is the trRev binding
        /// dependency — unused in Rust, it only forces QML re-evaluation
        /// when the language changes live.
        #[qinvokable]
        fn tr(self: &QbzSession, msgid: QString, rev: i32) -> QString;
        /// Ask for `path` (a file:// cover) re-encoded to fit w*h DEVICE
        /// pixels, aspect preserved. Answers on `artScaledReady`; a
        /// derivative already on disk answers on the next event-loop pass.
        #[qinvokable]
        fn art_scaled(self: Pin<&mut QbzSession>, path: QString, w: i32, h: i32);
        /// Synchronous WARM-CACHE probe for the exact derivative request.
        /// This never decodes or writes: it returns the existing local file,
        /// or "" so QML can fall through to the asynchronous arm above.
        #[qinvokable]
        fn art_scaled_cached(self: &QbzSession, path: QString, w: i32, h: i32) -> QString;
        /// (requested path, derivative url, requested w, requested h) — keyed
        /// by the WHOLE REQUEST. The path alone is not a key: it is normal for
        /// the same cover to be on screen at two sizes at once (a 200px card
        /// and the 36px row under it, a rail card and the now-playing dock),
        /// and with a path-only key each listener accepted the other's answer
        /// — the small one then point-sampled a card-sized file and the large
        /// one blew up a thumbnail. Measured on the 44px slim thumbnail
        /// accepting a 200x200 derivative: RMSE 10.4 against a Lanczos
        /// reference, i.e. worse than doing nothing.
        #[qsignal]
        fn art_scaled_ready(
            self: Pin<&mut QbzSession>,
            path: QString,
            scaled: QString,
            w: i32,
            h: i32,
        );
    }

    impl cxx_qt::Threading for QbzSession {}
}

use qbz_session::QbzSession;

/// Rust side of the session bridge (plain storage, phase-1 pattern).
pub struct QbzSessionRust {
    screen: QString,
    login_phase: i32,
    login_error: QString,
    restore_error: QString,
    offline: bool,
    offline_mode: i32,
    connectivity: i32,
    captive_portal: bool,
    has_previous_session: bool,
    show_recovery_banner: bool,
    offline_session: bool,
    session_user_name: QString,
    session_subscription: QString,
    tr_rev: i32,
}

impl Default for QbzSessionRust {
    fn default() -> Self {
        Self {
            screen: QString::from("splash"),
            login_phase: 0,
            login_error: QString::default(),
            restore_error: QString::default(),
            offline: false,
            offline_mode: 0,
            connectivity: 0,
            captive_portal: false,
            has_previous_session: false,
            show_recovery_banner: false,
            offline_session: false,
            session_user_name: QString::default(),
            session_subscription: QString::default(),
            tr_rev: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// The UI hop (bridges/mod.rs pattern)
// ---------------------------------------------------------------------------

static QT_THREAD: OnceLock<CxxQtThread<QbzSession>> = OnceLock::new();

/// Queue a session-bridge mutation onto the Qt event loop (no-op before
/// boot registers the thread).
pub(crate) fn ui(f: impl FnOnce(Pin<&mut QbzSession>) + Send + 'static) {
    if let Some(thread) = QT_THREAD.get() {
        let _ = thread.queue(f);
    }
}

impl qbz_session::QbzSession {
    pub fn boot(self: Pin<&mut Self>) {
        if QT_THREAD.set(self.qt_thread()).is_err() {
            log::warn!("[qbz-qt] session Qt thread already registered");
        }
        crate::on_boot();
    }

    pub fn sign_in_via_browser(self: Pin<&mut Self>) {
        crate::start_login();
    }

    pub fn cancel_login(self: Pin<&mut Self>) {
        crate::cancel_login();
    }

    pub fn start_offline(self: Pin<&mut Self>) {
        crate::start_offline();
    }

    pub fn recovery_login(self: Pin<&mut Self>) {
        crate::start_login();
    }

    pub fn open_tos(self: Pin<&mut Self>) {
        // Same URL the Slint shell opens (crates/qbz/src/main.rs
        // QOBUZ_TOS_URL). `open` spawns xdg-open detached, so this is safe
        // to run on the Qt thread.
        if let Err(e) = open::that(crate::QOBUZ_TOS_URL) {
            log::error!("[qbz-qt] failed to open Terms of Service: {e}");
        }
    }

    pub fn logout(self: Pin<&mut Self>) {
        crate::do_logout();
    }

    pub fn toggle_profile(self: Pin<&mut Self>) {
        crate::kiosk_profile_qt::toggle();
    }

    pub fn tr(&self, msgid: QString, rev: i32) -> QString {
        let _ = rev; // binding dependency only (see the bridge declaration)
        QString::from(&qbz_i18n::t(&msgid.to_string()))
    }
    pub fn art_scaled(self: Pin<&mut Self>, path: QString, w: i32, h: i32) {
        let (p, rw, rh) = (path.to_string(), w.max(0) as u32, h.max(0) as u32);
        crate::spawn(async move {
            let key = p.clone();
            let out =
                tokio::task::spawn_blocking(move || crate::artwork_qt::scaled_path(&p, rw, rh))
                    .await
                    .ok()
                    .flatten();
            if let Some(out) = out {
                let url = crate::artwork_qt::file_url(&out.to_string_lossy());
                ui(move |mut b| {
                    // The requested size rides back with the answer: see the
                    // signal's declaration — the path alone does not identify
                    // the request.
                    b.as_mut().art_scaled_ready(
                        QString::from(key.as_str()),
                        QString::from(url.as_str()),
                        w,
                        h,
                    );
                });
            }
        });
    }

    pub fn art_scaled_cached(&self, path: QString, w: i32, h: i32) -> QString {
        if w <= 0 || h <= 0 {
            return QString::default();
        }
        crate::artwork_qt::cached_scaled_path(&path.to_string(), w as u32, h as u32)
            .map(|p| QString::from(crate::artwork_qt::file_url(&p.to_string_lossy()).as_str()))
            .unwrap_or_default()
    }
}
